use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
}

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            agents: Mutex::new(HashMap::new()),
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
    pub fn add_agent(&self, provider_id: &str, agent: Arc<dyn CodingAgent>) {
        let mut map = self.agents.lock().expect("registry lock poisoned");
        map.insert(provider_id.to_string(), agent);
    }

    /// Remove an agent (called after DB delete).
    pub fn remove_agent(&self, provider_id: &str) {
        let mut map = self.agents.lock().expect("registry lock poisoned");
        map.remove(provider_id);
    }

    /// Return count of loaded agents.
    pub fn count(&self) -> usize {
        let map = self.agents.lock().expect("registry lock poisoned");
        map.len()
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
}
