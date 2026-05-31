use crate::db::Db;
use crate::error::AppError;
use chrono::Utc;
use serde::Serialize;
use tracing::info;
use uuid::Uuid;

/// Domain representation of a provider row.
#[derive(Debug, Clone, Serialize)]
pub struct Provider {
    pub id: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub name: String,
    #[serde(skip_serializing)]
    pub config_json: String,
    pub created_at: String,
}

/// Stateless store for provider persistence.
///
/// All methods take `&Db` — no connection pooling, no lifetime management.
/// The caller holds the `MutexGuard` for the duration of each method call.
pub struct ProviderStore;

impl ProviderStore {
    /// Insert a new provider. Returns the created row.
    pub fn create(
        db: &Db,
        provider_type: &str,
        name: &str,
        config_json: &str,
    ) -> Result<Provider, AppError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        db.conn()
            .query_row(
                "INSERT INTO providers (id, type, name, config_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 RETURNING id, type, name, config_json, created_at",
                rusqlite::params![id, provider_type, name, config_json, now],
                Self::map_row,
            )
            .map_err(AppError::from)
    }

    /// Fetch a provider by primary key.
    pub fn get_by_id(db: &Db, id: &str) -> Result<Provider, AppError> {
        db.conn()
            .query_row(
                "SELECT id, type, name, config_json, created_at
                 FROM providers WHERE id = ?1",
                [id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "provider".into(),
                    id: id.into(),
                },
                other => other.into(),
            })
    }

    /// List all providers (no pagination — low cardinality).
    pub fn list(db: &Db) -> Result<Vec<Provider>, AppError> {
        let conn = db.conn();
        let mut stmt = conn.prepare(
            "SELECT id, type, name, config_json, created_at
             FROM providers
             ORDER BY created_at ASC",
        )?;

        let rows: Vec<Provider> = stmt
            .query_map([], Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Hard delete a provider.
    pub fn delete(db: &Db, id: &str) -> Result<(), AppError> {
        let rows_affected = db
            .conn()
            .execute("DELETE FROM providers WHERE id = ?1", [id])?;

        if rows_affected == 0 {
            return Err(AppError::NotFound {
                resource: "provider".into(),
                id: id.into(),
            });
        }

        info!(provider_id = %id, "Provider deleted");
        Ok(())
    }

    /// Check if any sessions reference this provider.
    pub fn has_sessions(db: &Db, provider_id: &str) -> Result<bool, AppError> {
        let count: i64 = db.conn().query_row(
            "SELECT COUNT(*) FROM sessions WHERE provider_id = ?1",
            [provider_id],
            |r| r.get(0),
        )?;

        Ok(count > 0)
    }

    /// Map a result row to a `Provider`.
    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Provider> {
        Ok(Provider {
            id: row.get(0)?,
            provider_type: row.get(1)?,
            name: row.get(2)?,
            config_json: row.get(3)?,
            created_at: row.get(4)?,
        })
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
    fn test_create_provider() {
        let db = test_db();
        let config = sample_config();
        let provider = ProviderStore::create(&db, "anthropic", "My Anthropic", &config).unwrap();

        assert!(!provider.id.is_empty());
        assert_eq!(provider.provider_type, "anthropic");
        assert_eq!(provider.name, "My Anthropic");
        assert_eq!(provider.config_json, config);
        assert!(!provider.created_at.is_empty());
    }

    #[test]
    fn test_get_by_id() {
        let db = test_db();
        let config = sample_config();
        let created = ProviderStore::create(&db, "anthropic", "Test", &config).unwrap();
        let fetched = ProviderStore::get_by_id(&db, &created.id).unwrap();

        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, "Test");
        assert_eq!(fetched.config_json, config);
    }

    #[test]
    fn test_get_by_id_not_found() {
        let db = test_db();
        let result = ProviderStore::get_by_id(&db, "nonexistent");

        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { resource, id } => {
                assert_eq!(resource, "provider");
                assert_eq!(id, "nonexistent");
            }
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_list_empty() {
        let db = test_db();
        let providers = ProviderStore::list(&db).unwrap();
        assert!(providers.is_empty());
    }

    #[test]
    fn test_list_multiple() {
        let db = test_db();
        let config = sample_config();
        ProviderStore::create(&db, "anthropic", "First", &config).unwrap();
        ProviderStore::create(&db, "anthropic", "Second", &config).unwrap();
        ProviderStore::create(&db, "anthropic", "Third", &config).unwrap();

        let providers = ProviderStore::list(&db).unwrap();
        assert_eq!(providers.len(), 3);
    }

    #[test]
    fn test_delete() {
        let db = test_db();
        let config = sample_config();
        let created = ProviderStore::create(&db, "anthropic", "To Delete", &config).unwrap();
        ProviderStore::delete(&db, &created.id).unwrap();

        let result = ProviderStore::get_by_id(&db, &created.id);
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_not_found() {
        let db = test_db();
        let result = ProviderStore::delete(&db, "nonexistent");

        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { .. } => {}
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_has_sessions_false() {
        let db = test_db();
        let config = sample_config();
        let provider = ProviderStore::create(&db, "anthropic", "Test", &config).unwrap();

        let has = ProviderStore::has_sessions(&db, &provider.id).unwrap();
        assert!(!has);
    }

    #[test]
    fn test_has_sessions_true() {
        let db = test_db();
        let config = sample_config();
        let provider = ProviderStore::create(&db, "anthropic", "Test", &config).unwrap();

        // Insert a workspace (required for FK) and a session referencing this provider
        let ws_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'test', 'active', ?2, ?2)",
                rusqlite::params![ws_id, now],
            )
            .unwrap();

        let session_id = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO sessions (id, workspace_id, provider_id, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'connecting', ?4, ?4)",
                rusqlite::params![session_id, ws_id, provider.id, now],
            )
            .unwrap();

        let has = ProviderStore::has_sessions(&db, &provider.id).unwrap();
        assert!(has);
    }
}
