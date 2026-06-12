use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future;
use serde::Deserialize;
use tracing::{info, warn};

use crate::agent::anthropic::AnthropicAgent;
use crate::agent::claude_code::ClaudeCodeCodingAgent;
use crate::agent::model_cache::ModelCache;
use crate::agent::CodingAgent;
use crate::db::Db;
use crate::error::{AppError, ProviderError};
use crate::service::ActiveChildProcesses;
use crate::store::providers::ProviderStore;

/// Per-provider config extracted from `config_json` for agent
/// construction (feat-039 widening). The HTTP shape is the pre-
/// existing 3-field object; the CLI shape carries binary / args /
/// env / permission mode. Deserialized as untagged so a single
/// `config_json` blob is parsed into the variant matching its
/// fields.
///
/// `Http` and `Cli` are mutually exclusive: the `create_provider`
/// API handler validates the per-kind invariant and rejects
/// mixed-field payloads with a 400. The `untagged` deserializer
/// here is the read-side mirror of that validation.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ProviderConfig {
    /// HTTP provider shape: `{ base_url, api_key, default_model }`.
    Http {
        base_url: String,
        api_key: String,
        default_model: String,
    },
    /// CLI provider shape: `{ default_model, binary_path, args_json,
    /// env_json, permission_mode }`. All fields required by the
    /// `create_cli_provider` handler. Args and env are JSON strings
    /// (stored in the row's dedicated columns too, but the config
    /// keeps a copy for round-trip / debug log consistency).
    Cli {
        #[allow(dead_code)]
        default_model: String,
        #[allow(dead_code)]
        binary_path: String,
        #[allow(dead_code)]
        #[serde(default)]
        args_json: String,
        #[allow(dead_code)]
        #[serde(default)]
        env_json: String,
        #[allow(dead_code)]
        permission_mode: String,
    },
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
    /// 5-min-TTL cache of per-provider model lists (feat-042). Mirrors
    /// `HealthCache` discipline: invalidated from `add_agent` and
    /// `remove_agent` so the cache never holds a stale entry for a
    /// provider that just left the registry.
    model_cache: ModelCache,
    /// Per-session metadata populated by `ClaudeCodeCodingAgent` at
    /// end-of-stream and consumed by `run_prompt_task` after the
    /// `agent_loop` returns. Keyed by `session_id`. Entries are
    /// `take`-then-clear so a re-read returns `None` (the next turn
    /// starts fresh). Held as an `Arc<Mutex<…>>` so the same
    /// allocation is shared with every `ClaudeCodeCodingAgent` the
    /// registry constructs (the agent's spawned parser/translator
    /// task holds an `Arc` clone and writes into it at end-of-stream).
    /// The accessor `take_turn_outcome` locks the inner `Mutex`
    /// through the `Arc` — same allocation as the agents see.
    turn_outcomes: Arc<Mutex<HashMap<String, TurnOutcome>>>,
    /// Shared CLI child pid table (feat-049). The `CliRunner` inside
    /// every `ClaudeCodeCodingAgent` writes to this; the HTTP cancel
    /// handler and the cold-start reaper read from it. Kept on
    /// the registry so `create_agent` can thread it into the
    /// `CliRunner::with_registry` constructor without a separate
    /// plumbing path.
    active_child_processes: Arc<ActiveChildProcesses>,
}

/// Per-turn metadata surfaced by `ClaudeCodeCodingAgent` (feat-051).
/// See `agent::claude_code::agent::TurnOutcome` for the field-level
/// docs; this is the registry's view of the same shape (re-exported
/// here so callers don't have to depend on the `claude_code` module
/// to read the registry).
#[derive(Debug, Default, Clone)]
pub struct TurnOutcome {
    pub captured_cli_resume_id: Option<String>,
    pub did_reject: bool,
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

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    /// Create an empty registry with a fresh `ActiveChildProcesses`
    /// table. Use this in tests; use [`ProviderRegistry::with_shared_process_registry`]
    /// in production to share the AppState-level table.
    pub fn new() -> Self {
        Self {
            agents: Mutex::new(HashMap::new()),
            health_cache: Mutex::new(HealthCache {
                fetched_at: None,
                snapshot: Vec::new(),
            }),
            model_cache: ModelCache::new(),
            turn_outcomes: Arc::new(Mutex::new(HashMap::new())),
            active_child_processes: Arc::new(ActiveChildProcesses::new()),
        }
    }

    /// Build a registry that shares the given `ActiveChildProcesses`
    /// table. Production wiring: `AppState` constructs one
    /// `ActiveChildProcesses` and shares it across the
    /// `ProviderRegistry` and the cancel handler / reaper (feat-049).
    pub fn with_shared_process_registry(registry: Arc<ActiveChildProcesses>) -> Self {
        Self {
            agents: Mutex::new(HashMap::new()),
            health_cache: Mutex::new(HealthCache {
                fetched_at: None,
                snapshot: Vec::new(),
            }),
            model_cache: ModelCache::new(),
            turn_outcomes: Arc::new(Mutex::new(HashMap::new())),
            active_child_processes: registry,
        }
    }

    /// Borrow the shared `ActiveChildProcesses` registry. The
    /// `CliRunner` inside each `ClaudeCodeCodingAgent` already
    /// holds an `Arc` clone; this is the escape hatch for tests
    /// that want to assert on the pid table directly.
    pub fn active_child_processes(&self) -> Arc<ActiveChildProcesses> {
        Arc::clone(&self.active_child_processes)
    }

    /// Take the per-session turn outcome (feat-051). Removes the
    /// entry on read so the next turn starts fresh. Returns
    /// `TurnOutcome::default()` (both fields `None` / `false`) for
    /// HTTP runtimes, for first turns before the agent has had a
    /// chance to populate the map, and for `session_id`s the agent
    /// never touched.
    pub fn take_turn_outcome(&self, session_id: &str) -> TurnOutcome {
        let mut map = self
            .turn_outcomes
            .lock()
            .expect("turn outcomes lock poisoned");
        map.remove(session_id).unwrap_or_default()
    }

    /// Borrow the shared `turn_outcomes` map as an `Arc<Mutex<…>>`
    /// (feat-051). The `ClaudeCodeCodingAgent` constructor takes
    /// one of these so its spawned parser/translator task can write
    /// outcomes into the SAME map the `run_prompt_task` consumer
    /// reads from via `take_turn_outcome`. Production wiring
    /// (`api/providers::create_cli_provider`) calls
    /// `turn_outcomes_arc()` on the AppState's registry and passes
    /// the `Arc` clone into the agent. Tests construct a fresh
    /// registry and pass the same `Arc` to both the agent and the
    /// reader side.
    pub fn turn_outcomes_arc(&self) -> Arc<Mutex<HashMap<String, TurnOutcome>>> {
        Arc::clone(&self.turn_outcomes)
    }

    /// Load all providers from DB and construct agents.
    ///
    /// Logs warnings for providers that fail to construct; does not abort.
    /// Returns the count of successfully loaded agents.
    pub fn load_from_db(&self, db: &Db) -> Result<u32, AppError> {
        let providers = ProviderStore::list(db)?;
        let mut map = self.agents.lock().expect("registry lock poisoned");
        let mut loaded = 0u32;
        let turn_outcomes = Arc::clone(&self.turn_outcomes);

        for provider in &providers {
            // Per-kind config. The `ProviderConfig` enum's untagged
            // deserialize handles both shapes; CLI rows serialize
            // `{"default_model": ...}` to the legacy `config_json`
            // column (per `ProviderStore::create_cli`), so a CLI
            // load-from-db on the legacy column would parse as
            // `Http` and miss the `binary_path` field — so we
            // thread the structured per-kind fields directly
            // instead of going through `config_json` for CLI rows.
            let result = match provider.kind.as_str() {
                "cli" => Self::create_cli_agent_from_row(provider, Arc::clone(&turn_outcomes)),
                _ => Self::create_agent(&provider.provider_type, &provider.config_json),
            };
            match result {
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
    /// for up to 10s). Also invalidates the model cache (feat-042) so
    /// a stale entry from a prior provider row with the same id can
    /// never leak through.
    pub fn add_agent(&self, provider_id: &str, agent: Arc<dyn CodingAgent>) {
        let mut map = self.agents.lock().expect("registry lock poisoned");
        map.insert(provider_id.to_string(), agent);
        drop(map);
        self.invalidate_health_cache();
        self.model_cache.invalidate(provider_id);
    }

    /// Remove an agent (called after DB delete).
    ///
    /// Invalidates the health cache so the next `cached_health_summary`
    /// call re-probes (otherwise the removed provider would still
    /// appear in `total` for up to 10s). Also invalidates the model
    /// cache (feat-042) so the removed provider's cached model list
    /// does not outlive its row.
    pub fn remove_agent(&self, provider_id: &str) {
        let mut map = self.agents.lock().expect("registry lock poisoned");
        map.remove(provider_id);
        drop(map);
        self.invalidate_health_cache();
        self.model_cache.invalidate(provider_id);
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

    /// Return the most recently probed health for `provider_id` from
    /// the 10s `HealthCache` (feat-053). Returns `false` when the id
    /// was never probed in this process lifetime (cold start) or when
    /// the cache has been invalidated. Does **not** trigger a probe —
    /// the wizard's Step 1 keeps a provider greyed until the first
    /// `cached_health_summary` call (typically from the
    /// `/api/health` aggregate endpoint) warms the cache, rather
    /// than blocking the list response on per-id I/O.
    ///
    /// Locking: takes the `health_cache` mutex once, clones the
    /// snapshot out, and iterates the clone — the lock window is the
    /// clone only, not the lookup. `list_providers` (the only caller)
    /// holds the snapshot across an N-row loop without re-locking.
    pub fn cached_health_for(&self, provider_id: &str) -> bool {
        let snapshot = {
            let cache = self
                .health_cache
                .lock()
                .expect("health cache lock poisoned");
            // If the cache has never been warmed, every provider is
            // "unseen" — return false for all of them. This mirrors
            // the existing `cached_health_summary` aggregate behavior
            // (uncached ids simply do not appear in the snapshot).
            if cache.fetched_at.is_none() {
                return false;
            }
            cache.snapshot.clone()
        };
        snapshot
            .iter()
            .find(|(id, _)| id == provider_id)
            .map(|(_, healthy)| *healthy)
            .unwrap_or(false)
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

    // ---- Model cache accessors (feat-042) ----

    /// Look up the cached model list for `provider_id`. Returns
    /// `Some((models, fresh))` if an entry exists, `None` otherwise.
    /// `fresh` is `true` when the entry is younger than the cache TTL.
    pub(crate) fn get_cached_models(
        &self,
        provider_id: &str,
    ) -> Option<(Vec<crate::agent::ModelInfo>, bool)> {
        self.model_cache.get(provider_id)
    }

    /// Store the freshly-fetched model list for `provider_id`. Overwrites
    /// any existing entry.
    pub(crate) fn put_cached_models(
        &self,
        provider_id: &str,
        models: Vec<crate::agent::ModelInfo>,
    ) {
        self.model_cache.put(provider_id, models);
    }

    /// Invalidate the model-cache entry for `provider_id`. No-op if
    /// absent. Exposed for the explicit `POST .../models` refresh
    /// endpoint, which wants to drop a stale entry before re-fetching.
    pub(crate) fn invalidate_models(&self, provider_id: &str) {
        self.model_cache.invalidate(provider_id);
    }

    /// Create an agent instance from provider type and config_json.
    pub(crate) fn create_agent(
        provider_type: &str,
        config_json: &str,
    ) -> Result<Arc<dyn CodingAgent>, ProviderError> {
        let config: ProviderConfig = serde_json::from_str(config_json)
            .map_err(|e| ProviderError::Unreachable(format!("invalid config_json: {e}")))?;

        match (provider_type, config) {
            (
                "anthropic",
                ProviderConfig::Http {
                    base_url,
                    api_key,
                    default_model,
                },
            ) => {
                let agent = AnthropicAgent::new(base_url, api_key, default_model)?;
                Ok(Arc::new(agent))
            }
            (other, _) => Err(ProviderError::Unreachable(format!(
                "unsupported provider type or config shape for kind=http: {other}"
            ))),
        }
    }

    /// Build a CLI agent directly from a `Provider` row's
    /// dedicated CLI columns (binary_path, args_json, env_json,
    /// permission_mode, default_model). This bypasses
    /// `create_agent`'s `config_json` deserialization because the
    /// legacy column carries only `{"default_model": ...}` for
    /// CLI rows.
    fn create_cli_agent_from_row(
        provider: &crate::store::providers::Provider,
        turn_outcomes: Arc<Mutex<HashMap<String, TurnOutcome>>>,
    ) -> Result<Arc<dyn CodingAgent>, ProviderError> {
        // The `provider_type` column on the `Provider` row is the
        // agent-family discriminator: `"anthropic"` for HTTP rows
        // (only family in v1) and the per-CLI-adapter family name
        // (`"claude-code"` today) for CLI rows. The CLI-row
        // dispatch is by binary basename: `claude` → `ClaudeCodeCodingAgent`.
        // Matching is by basename, not the full path, so a row
        // pointing at `/opt/.../claude` (a symlink or alt install)
        // still routes to the right agent. The `provider_type`
        // value is essentially decorative for CLI rows — the row's
        // stored value (e.g. `"claude-code"`) is the value the
        // `CodingAgent::provider_type()` method returns, but the
        // dispatcher doesn't read it. Future CLIs (Codex, OpenCode)
        // get their own match arms and their own family names.
        let binary_path_str = provider
            .binary_path
            .as_deref()
            .ok_or_else(|| ProviderError::Unreachable("CLI provider missing binary_path".into()))?;
        let basename = std::path::Path::new(binary_path_str)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        match basename {
            "claude" | "fake_cli" => {
                let binary_path = std::path::PathBuf::from(binary_path_str);
                let args_str = provider.args_json.as_deref().unwrap_or("[]");
                let args: Vec<String> = serde_json::from_str(args_str)
                    .map_err(|e| ProviderError::Unreachable(format!("invalid args_json: {e}")))?;
                let env_str = provider.env_json.as_deref().unwrap_or("{}");
                let env: std::collections::BTreeMap<String, String> = serde_json::from_str(env_str)
                    .map_err(|e| ProviderError::Unreachable(format!("invalid env_json: {e}")))?;
                let default_model = provider
                    .default_model
                    .clone()
                    .unwrap_or_else(|| "claude-sonnet-4-5".to_string());
                let permission_mode = provider
                    .permission_mode
                    .clone()
                    .unwrap_or_else(|| "default".to_string());
                // Cold-start reload path: the load_from_db
                // constructor builds a fresh `ActiveChildProcesses`
                // (the AppState-level one is not in scope here).
                // The HTTP-create path (`api/providers::create_cli_provider`)
                // uses the AppState's shared registry instead so
                // cancel and reaper see the same pid table.
                let registry = Arc::new(ActiveChildProcesses::new());
                // The `turn_outcomes` map is shared with the
                // reader side via the `Arc<Mutex<…>>` accessor on
                // `ProviderRegistry` — `take_turn_outcome` locks
                // the same allocation. `load_from_db` passes the
                // `Arc` in so reloads share the same map.
                let agent = ClaudeCodeCodingAgent::new(
                    binary_path,
                    args,
                    env,
                    default_model,
                    permission_mode,
                    registry,
                    turn_outcomes,
                );
                Ok(Arc::new(agent))
            }
            other => Err(ProviderError::Unreachable(format!(
                "unsupported CLI binary basename: {other}"
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

    /// 3. (feat-042 verification) `add_agent` and `remove_agent` both
    /// invalidate the model-cache entry for the touched provider. This
    /// is the symmetry test: put an entry, mutate the registry, the
    /// entry must be gone. Without this wiring, a stale model list
    /// from a prior provider row with the same id would leak through
    /// for up to 5 minutes after the row was deleted.
    #[test]
    fn test_model_cache_invalidation_on_add_remove() {
        use crate::agent::ModelInfo;

        let registry = ProviderRegistry::new();
        let agent = AnthropicAgent::new(
            "https://api.anthropic.com".into(),
            "sk-test".into(),
            "claude-sonnet-4-20250514".into(),
        )
        .unwrap();

        // Baseline: cache is empty.
        assert!(registry.get_cached_models("p1").is_none());

        // Put an entry directly via the registry accessor.
        registry.put_cached_models(
            "p1",
            vec![ModelInfo {
                id: "stale".into(),
                name: "Stale".into(),
                context_window: 1,
            }],
        );
        assert!(registry.get_cached_models("p1").is_some());

        // add_agent must drop the entry.
        registry.add_agent("p1", Arc::new(agent));
        assert!(
            registry.get_cached_models("p1").is_none(),
            "add_agent must invalidate the model cache"
        );

        // Re-populate, then remove_agent must also drop the entry.
        registry.put_cached_models(
            "p1",
            vec![ModelInfo {
                id: "stale-2".into(),
                name: "Stale 2".into(),
                context_window: 2,
            }],
        );
        assert!(registry.get_cached_models("p1").is_some());

        registry.remove_agent("p1");
        assert!(
            registry.get_cached_models("p1").is_none(),
            "remove_agent must invalidate the model cache"
        );
    }
}
