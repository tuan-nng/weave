use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future;
use serde::Deserialize;
use tracing::{info, warn};

use crate::agent::anthropic::AnthropicAgent;
use crate::agent::CodingAgent;
use crate::db::Db;
use crate::error::{AppError, ProviderError};
use crate::store::providers::ProviderStore;

/// Config fields extracted from `config_json` for agent construction.
#[derive(Debug, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key: String,
    pub default_model: String,
}

/// Thread-safe registry of live agent instances.
///
/// Wraps a `HashMap<String, Arc<dyn CodingAgent>>` behind a `Mutex`.
/// The lock is held only for HashMap lookups — no I/O inside critical sections.
pub struct ProviderRegistry {
    agents: Mutex<HashMap<String, Arc<dyn CodingAgent>>>,
    /// 10s-TTL cache of the last `cached_health_summary` call. Each
    /// `AppState` (and each test) gets its own cache via `new()`.
    health_cache: Mutex<HealthCache>,
}

/// Cached result of a `cached_health_summary` probe. `fetched_at` is
/// `None` until the first probe completes; `snapshot` mirrors the
/// agents map at probe time as `(provider_id, healthy)` pairs.
struct HealthCache {
    fetched_at: Option<Instant>,
    snapshot: Vec<(String, bool)>,
}

/// How long a cached health snapshot is considered fresh.
const HEALTH_CACHE_TTL: Duration = Duration::from_secs(10);

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            agents: Mutex::new(HashMap::new()),
            health_cache: Mutex::new(HealthCache {
                fetched_at: None,
                snapshot: Vec::new(),
            }),
        }
    }

    /// Load all providers from DB and construct agents.
    ///
    /// Logs warnings for providers that fail to construct; does not abort.
    /// Returns the count of successfully loaded agents.
    pub fn load_from_db(&self, db: &Db) -> Result<u32, AppError> {
        let providers = ProviderStore::list(db)?;
        let mut map = self.agents.lock().expect("registry lock poisoned");
        let mut loaded = 0u32;

        for provider in &providers {
            match Self::create_agent(&provider.provider_type, &provider.config_json) {
                Ok(agent) => {
                    map.insert(provider.id.clone(), agent);
                    loaded += 1;
                    info!(
                        provider_id = %provider.id,
                        name = %provider.name,
                        "Provider loaded"
                    );
                }
                Err(e) => {
                    warn!(
                        provider_id = %provider.id,
                        name = %provider.name,
                        error = %e,
                        "Failed to load provider, skipping"
                    );
                }
            }
        }

        Ok(loaded)
    }

    /// Get a live agent by provider ID.
    pub fn get_agent(&self, provider_id: &str) -> Result<Arc<dyn CodingAgent>, AppError> {
        let map = self.agents.lock().expect("registry lock poisoned");
        map.get(provider_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound {
                resource: "provider".into(),
                id: provider_id.into(),
            })
    }

    /// Register a new agent (called after DB insert).
    ///
    /// Invalidates the health cache so the next `cached_health_summary`
    /// call re-probes (otherwise the new provider would be invisible
    /// for up to 10s).
    pub fn add_agent(&self, provider_id: &str, agent: Arc<dyn CodingAgent>) {
        let mut map = self.agents.lock().expect("registry lock poisoned");
        map.insert(provider_id.to_string(), agent);
        self.invalidate_health_cache();
    }

    /// Remove an agent (called after DB delete).
    ///
    /// Invalidates the health cache so the next `cached_health_summary`
    /// call re-probes (otherwise the removed provider would still
    /// appear in `total` for up to 10s).
    pub fn remove_agent(&self, provider_id: &str) {
        let mut map = self.agents.lock().expect("registry lock poisoned");
        map.remove(provider_id);
        self.invalidate_health_cache();
    }

    /// Return count of loaded agents.
    pub fn count(&self) -> usize {
        let map = self.agents.lock().expect("registry lock poisoned");
        map.len()
    }

    /// Snapshot of `(provider_id, agent)` pairs.
    ///
    /// Clones the `HashMap` entries out under the lock so the caller can
    /// use them without holding the lock. The lock window is the HashMap
    /// iteration only — no I/O inside the critical section.
    fn agents_snapshot(&self) -> Vec<(String, Arc<dyn CodingAgent>)> {
        self.agents
            .lock()
            .expect("registry lock poisoned")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// `(total, healthy, unhealthy)` counts for the loaded agents,
    /// served from a 10s-TTL cache.
    ///
    /// On a cache miss, probes all agents **in parallel** via
    /// `futures_util::future::join_all`. Each agent's `health_check()`
    /// is wrapped to never panic — errors are folded into
    /// `healthy = false`. The result is cached for 10s; subsequent
    /// calls within the window return the cached snapshot with no I/O.
    pub async fn cached_health_summary(&self) -> (usize, usize, usize) {
        // Fast path: fresh cache hit
        if let Some(snap) = self.fresh_snapshot() {
            return summarize(&snap);
        }
        // Cold or stale: probe all agents in parallel
        let agents = self.agents_snapshot();
        let probed: Vec<(String, bool)> =
            future::join_all(agents.into_iter().map(|(id, agent)| async move {
                let healthy = agent
                    .health_check()
                    .await
                    .map(|h| h.healthy)
                    .unwrap_or(false);
                (id, healthy)
            }))
            .await;
        // Write cache (best-effort — ignore poisoning)
        if let Ok(mut cache) = self.health_cache.lock() {
            cache.snapshot = probed.clone();
            cache.fetched_at = Some(Instant::now());
        }
        summarize(&probed)
    }

    /// Return the cached snapshot if it exists AND is younger than
    /// [`HEALTH_CACHE_TTL`]. Otherwise `None`.
    fn fresh_snapshot(&self) -> Option<Vec<(String, bool)>> {
        let cache = self
            .health_cache
            .lock()
            .expect("health cache lock poisoned");
        cache
            .fetched_at
            .filter(|t| t.elapsed() < HEALTH_CACHE_TTL)
            .map(|_| cache.snapshot.clone())
    }

    /// Drop the health cache. Called on registry mutations so the next
    /// probe reflects the new state immediately rather than after
    /// [`HEALTH_CACHE_TTL`].
    fn invalidate_health_cache(&self) {
        if let Ok(mut cache) = self.health_cache.lock() {
            cache.fetched_at = None;
        }
    }

    /// Create an agent instance from provider type and config_json.
    pub(crate) fn create_agent(
        provider_type: &str,
        config_json: &str,
    ) -> Result<Arc<dyn CodingAgent>, ProviderError> {
        let config: ProviderConfig = serde_json::from_str(config_json)
            .map_err(|e| ProviderError::Unreachable(format!("invalid config_json: {e}")))?;

        match provider_type {
            "anthropic" => {
                let agent =
                    AnthropicAgent::new(config.base_url, config.api_key, config.default_model)?;
                Ok(Arc::new(agent))
            }
            other => Err(ProviderError::Unreachable(format!(
                "unsupported provider type: {other}"
            ))),
        }
    }
}

/// Reduce a `(provider_id, healthy)` slice to `(total, healthy, unhealthy)`.
fn summarize(snap: &[(String, bool)]) -> (usize, usize, usize) {
    let total = snap.len();
    let healthy = snap.iter().filter(|(_, h)| *h).count();
    (total, healthy, total - healthy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_db() -> Db {
        Db::open(Path::new(":memory:")).expect("failed to open test db")
    }

    fn sample_config() -> String {
        serde_json::json!({
            "base_url": "https://api.anthropic.com",
            "api_key": "sk-test-123",
            "default_model": "claude-sonnet-4-20250514"
        })
        .to_string()
    }

    #[test]
    fn test_new_registry_is_empty() {
        let registry = ProviderRegistry::new();
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_add_and_get_agent() {
        let registry = ProviderRegistry::new();
        let agent = AnthropicAgent::new(
            "https://api.anthropic.com".into(),
            "sk-test".into(),
            "claude-sonnet-4-20250514".into(),
        )
        .unwrap();

        registry.add_agent("test-id", Arc::new(agent));
        assert_eq!(registry.count(), 1);

        let retrieved = registry.get_agent("test-id").unwrap();
        assert_eq!(retrieved.provider_type(), "anthropic");
    }

    #[test]
    fn test_get_agent_not_found() {
        let registry = ProviderRegistry::new();
        let result = registry.get_agent("nonexistent");

        match result {
            Err(AppError::NotFound { resource, id }) => {
                assert_eq!(resource, "provider");
                assert_eq!(id, "nonexistent");
            }
            Err(e) => panic!("expected NotFound, got error: {}", e),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn test_remove_agent() {
        let registry = ProviderRegistry::new();
        let agent = AnthropicAgent::new(
            "https://api.anthropic.com".into(),
            "sk-test".into(),
            "claude-sonnet-4-20250514".into(),
        )
        .unwrap();

        registry.add_agent("test-id", Arc::new(agent));
        assert_eq!(registry.count(), 1);

        registry.remove_agent("test-id");
        assert_eq!(registry.count(), 0);

        let result = registry.get_agent("test-id");
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_nonexistent_agent() {
        let registry = ProviderRegistry::new();
        registry.remove_agent("nonexistent"); // no-op, no panic
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_load_from_db_empty() {
        let db = test_db();
        let registry = ProviderRegistry::new();
        let loaded = registry.load_from_db(&db).unwrap();
        assert_eq!(loaded, 0);
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_load_from_db_with_valid_provider() {
        let db = test_db();
        let config = sample_config();
        ProviderStore::create(&db, "anthropic", "Test", &config).unwrap();

        let registry = ProviderRegistry::new();
        let loaded = registry.load_from_db(&db).unwrap();
        assert_eq!(loaded, 1);
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn test_load_from_db_skips_bad_config() {
        let db = test_db();
        // Insert provider with invalid config_json
        ProviderStore::create(&db, "anthropic", "Bad Config", "not-json").unwrap();

        let registry = ProviderRegistry::new();
        let loaded = registry.load_from_db(&db).unwrap();
        assert_eq!(loaded, 0, "should skip provider with invalid config");
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_create_agent_anthropic() {
        let config = sample_config();
        let agent = ProviderRegistry::create_agent("anthropic", &config);
        assert!(agent.is_ok());
        assert_eq!(agent.unwrap().provider_type(), "anthropic");
    }

    #[test]
    fn test_create_agent_unsupported_type() {
        let config = sample_config();
        let result = ProviderRegistry::create_agent("openai", &config);
        assert!(result.is_err(), "unsupported type should return error");
    }

    #[test]
    fn test_create_agent_missing_fields() {
        let result = ProviderRegistry::create_agent("anthropic", "{}");
        assert!(result.is_err(), "missing fields should return error");
    }

    /// `agents_snapshot` returns an empty vec for an empty registry and
    /// does not hold the lock during any await.
    #[test]
    fn test_agents_snapshot_returns_empty_for_empty_registry() {
        let registry = ProviderRegistry::new();
        assert!(registry.agents_snapshot().is_empty());
    }

    /// `cached_health_summary` on an empty registry returns `(0, 0, 0)`
    /// without ever calling a network endpoint.
    #[tokio::test]
    async fn test_cached_health_summary_empty_registry() {
        let registry = ProviderRegistry::new();
        let (total, healthy, unhealthy) = registry.cached_health_summary().await;
        assert_eq!((total, healthy, unhealthy), (0, 0, 0));
    }

    /// A second `cached_health_summary` call within the TTL window
    /// reuses the cached snapshot — `health_check` is NOT re-invoked.
    /// Verified via a stub agent that counts its invocations.
    #[tokio::test]
    async fn test_cached_health_summary_cache_hit_within_ttl() {
        use crate::agent::ProviderHealth;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct StubAgent {
            calls: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl crate::agent::CodingAgent for StubAgent {
            fn provider_type(&self) -> &str {
                "stub"
            }
            fn display_name(&self) -> &str {
                "stub"
            }
            async fn list_models(&self) -> Result<Vec<crate::agent::ModelInfo>, ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _req: crate::agent::MessageRequest,
                _turn: &crate::agent::turn_context::TurnContext,
            ) -> Result<
                std::pin::Pin<
                    Box<
                        dyn futures_util::Stream<
                                Item = Result<crate::agent::StreamEvent, ProviderError>,
                            > + Send,
                    >,
                >,
                ProviderError,
            > {
                Err(ProviderError::Unreachable("stub".into()))
            }
            async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(ProviderHealth {
                    healthy: true,
                    latency_ms: 1,
                    error: None,
                })
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let agent = Arc::new(StubAgent {
            calls: calls.clone(),
        });
        let registry = ProviderRegistry::new();
        registry.add_agent("p1", agent);

        // First call: probes, populates cache, increments calls to 1.
        let _ = registry.cached_health_summary().await;
        // Second call: cache hit, MUST NOT increment calls.
        let _ = registry.cached_health_summary().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "second call should hit cache"
        );

        // Invalidate via remove_agent; next call should re-probe.
        registry.remove_agent("p1");
        registry.add_agent(
            "p1",
            Arc::new(StubAgent {
                calls: calls.clone(),
            }),
        );
        let _ = registry.cached_health_summary().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "registry mutation should invalidate cache"
        );
    }
}
