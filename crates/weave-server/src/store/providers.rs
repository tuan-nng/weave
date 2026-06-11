use crate::db::Db;
use crate::error::AppError;
use chrono::Utc;
use serde::Serialize;
use tracing::info;
use uuid::Uuid;

/// Domain representation of a provider row.
///
/// feat-039 widens the row from "one HTTP shape" to a discriminated union
/// on `kind`. The HTTP shape is the pre-existing path; the CLI shape lets
/// us pre-register Claude Code / Codex / OpenCode providers before their
/// dispatch adapters land in feat-051.
///
/// feat-053 adds `healthy: bool` — populated by `list_providers` from the
/// in-memory `ProviderRegistry::cached_health_for` cache. The store layer
/// sets it to `false` (the cold-cache default); the handler enriches it
/// before serialization.
///
/// Field order in the struct mirrors the SELECT column order in `map_row`.
///
/// Wire shape (JSON):
///   * `id`, `type`, `kind`, `name`, `default_model`, `binary_path`,
///     `args_json`, `env_json`, `permission_mode`, `healthy`, `created_at`
///   * `config_json` is NEVER serialized (carries `api_key` for HTTP rows
///     and the canonical `{"default_model": ...}` wrapper for CLI rows)
///   * `api_key` is never present — HTTP rows get it via `config_json` only
///     and the response strips it; CLI rows never have it
#[derive(Debug, Clone, Serialize)]
pub struct Provider {
    pub id: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub kind: String,
    pub name: String,
    pub default_model: Option<String>,
    pub binary_path: Option<String>,
    pub args_json: Option<String>,
    pub env_json: Option<String>,
    pub permission_mode: Option<String>,
    /// feat-053: per-provider health snapshot from `ProviderRegistry`'s
    /// 10s `HealthCache`. `false` when the cache has never been warmed
    /// in this process lifetime (the conservative default; the wizard
    /// hides unproven providers rather than offering broken ones).
    /// The store layer always sets this to `false`; the handler in
    /// `api/providers.rs::list_providers` enriches it from the registry.
    pub healthy: bool,
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
    /// Insert a new HTTP provider. Returns the created row.
    ///
    /// For `kind="http"` rows the new `kind`, `default_model`, and CLI
    /// fields are populated. The `config_json` is whatever the caller
    /// built (typically `{"base_url":..., "api_key":..., "default_model":...}`).
    ///
    /// For `kind="cli"` rows, callers should use `create_cli` instead.
    pub fn create(
        db: &Db,
        provider_type: &str,
        name: &str,
        config_json: &str,
    ) -> Result<Provider, AppError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        // Best-effort extraction of `default_model` from the HTTP config
        // JSON so the wire field is populated for existing callers that
        // haven't been updated. For CLI rows the new `create_cli` writes
        // the canonical `{"default_model": ...}` wrapper.
        let default_model = extract_default_model_from_config(config_json);

        db.conn()
            .query_row(
                "INSERT INTO providers (
                    id, type, kind, name, default_model,
                    binary_path, args_json, env_json, permission_mode,
                    config_json, created_at
                 )
                 VALUES (?1, ?2, 'http', ?3, ?4, NULL, NULL, NULL, NULL, ?5, ?6)
                 RETURNING id, type, kind, name, default_model,
                           binary_path, args_json, env_json, permission_mode,
                           config_json, created_at",
                rusqlite::params![id, provider_type, name, default_model, config_json, now],
                Self::map_row,
            )
            .map_err(AppError::from)
    }

    /// Insert a new CLI provider. Returns the created row.
    ///
    /// Stores `config_json` as `{"default_model": <default_model>}` per the
    /// locked-in decision to keep the `service/sessions.rs:318`
    /// `default_model` extractor working for both kinds.
    pub fn create_cli(
        db: &Db,
        provider_type: &str,
        name: &str,
        default_model: &str,
        binary_path: &str,
        args_json: &str,
        env_json: &str,
        permission_mode: &str,
    ) -> Result<Provider, AppError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        // The legacy `config_json` column stays NOT NULL and carries
        // `{"default_model": ...}` for both kinds so the existing
        // `service/sessions.rs:318` extractor (which only reads
        // `default_model`) keeps working without code change.
        let config_json = serde_json::json!({
            "default_model": default_model,
        })
        .to_string();

        db.conn()
            .query_row(
                "INSERT INTO providers (
                    id, type, kind, name, default_model,
                    binary_path, args_json, env_json, permission_mode,
                    config_json, created_at
                 )
                 VALUES (
                    ?1, ?2, 'cli', ?3, ?4,
                    ?5, ?6, ?7, ?8,
                    ?9, ?10
                 )
                 RETURNING id, type, kind, name, default_model,
                           binary_path, args_json, env_json, permission_mode,
                           config_json, created_at",
                rusqlite::params![
                    id,
                    provider_type,
                    name,
                    default_model,
                    binary_path,
                    args_json,
                    env_json,
                    permission_mode,
                    config_json,
                    now,
                ],
                Self::map_row,
            )
            .map_err(AppError::from)
    }

    /// Fetch a provider by primary key.
    pub fn get_by_id(db: &Db, id: &str) -> Result<Provider, AppError> {
        db.conn()
            .query_row(
                "SELECT id, type, kind, name, default_model,
                        binary_path, args_json, env_json, permission_mode,
                        config_json, created_at
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
            "SELECT id, type, kind, name, default_model,
                    binary_path, args_json, env_json, permission_mode,
                    config_json, created_at
             FROM providers
             ORDER BY created_at ASC",
        )?;

        let rows: Vec<Provider> = stmt
            .query_map([], Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// List providers that match a given `RuntimeKind` (feat-056).
    ///
    /// The `providers.kind` column from feat-039 is a coarse
    /// discriminator with two values: `'http'` and `'cli'`. It does
    /// NOT carry the per-runtime breakdown — AnthropicApi,
    /// OpenaiApi, and OpenaiCompatible all share `kind='http'`,
    /// while ClaudeCode, Codex, and Opencode share `kind='cli'`.
    /// The mapping from `RuntimeKind` to the stored `kind` column
    /// lives here so the A2A resolution chokepoint has one
    /// canonical source. A future migration can add a
    /// `runtime_kind` column and tighten this filter; the API
    /// surface stays the same.
    ///
    /// `list_for_runtime` callers (A2A `messages.rs`) compose the
    /// result with `ProviderRegistry::cached_health_for` to drop
    /// cold-cache entries; that health filter is deliberately NOT
    /// applied here because the store layer has no access to the
    /// registry.
    ///
    /// Returns an empty `Vec` (not an error) when no providers exist
    /// for the given runtime — the caller decides whether that is a
    /// `no_provider_for_runtime` validation error or a soft miss.
    pub fn list_for_runtime(
        db: &Db,
        runtime: crate::agent::RuntimeKind,
    ) -> Result<Vec<Provider>, AppError> {
        let kind = match runtime {
            crate::agent::RuntimeKind::AnthropicApi
            | crate::agent::RuntimeKind::OpenaiApi
            | crate::agent::RuntimeKind::OpenaiCompatible => "http",
            crate::agent::RuntimeKind::ClaudeCode
            | crate::agent::RuntimeKind::Codex
            | crate::agent::RuntimeKind::Opencode => "cli",
        };
        let conn = db.conn();
        let mut stmt = conn.prepare(
            "SELECT id, type, kind, name, default_model,
                    binary_path, args_json, env_json, permission_mode,
                    config_json, created_at
             FROM providers
             WHERE kind = ?1
             ORDER BY created_at ASC",
        )?;

        let rows: Vec<Provider> = stmt
            .query_map([kind], Self::map_row)?
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
    ///
    /// Column order must match every SELECT and RETURNING clause above:
    ///   0  id
    ///   1  type
    ///   2  kind
    ///   3  name
    ///   4  default_model
    ///   5  binary_path
    ///   6  args_json
    ///   7  env_json
    ///   8  permission_mode
    ///   9  config_json   (skipped from wire serialization)
    ///   10 created_at
    ///
    /// `healthy` is NOT read from the database — the column does not
    /// exist there. The store layer writes `false` (the unseen-cache
    /// default); the API handler enriches from `ProviderRegistry`
    /// before serialization (feat-053).
    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Provider> {
        Ok(Provider {
            id: row.get(0)?,
            provider_type: row.get(1)?,
            kind: row.get(2)?,
            name: row.get(3)?,
            default_model: row.get(4)?,
            binary_path: row.get(5)?,
            args_json: row.get(6)?,
            env_json: row.get(7)?,
            permission_mode: row.get(8)?,
            healthy: false,
            config_json: row.get(9)?,
            created_at: row.get(10)?,
        })
    }
}

/// Best-effort extraction of `default_model` from a provider's HTTP
/// `config_json`. Returns `None` for malformed JSON or when the key is
/// absent. The same `service/sessions.rs:318` extractor (which uses
/// `serde_json::Value`) is the canonical read path; this helper is for
/// the write path on `ProviderStore::create`.
fn extract_default_model_from_config(config_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(config_json)
        .ok()
        .and_then(|v| {
            v.get("default_model")
                .and_then(|m| m.as_str())
                .map(String::from)
        })
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
        assert_eq!(provider.kind, "http");
        assert_eq!(provider.name, "My Anthropic");
        assert_eq!(
            provider.default_model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert!(provider.binary_path.is_none());
        assert!(provider.args_json.is_none());
        assert!(provider.env_json.is_none());
        assert!(provider.permission_mode.is_none());
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
        assert_eq!(fetched.kind, "http");
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
        assert!(providers.iter().all(|p| p.kind == "http"));
    }

    #[test]
    fn test_create_cli_provider() {
        let db = test_db();
        let provider = ProviderStore::create_cli(
            &db,
            "anthropic",
            "My Claude Code",
            "claude-sonnet-4-5",
            "/usr/local/bin/claude",
            r#"["--verbose"]"#,
            r#"{"LOG_LEVEL":"info"}"#,
            "accept-edits",
        )
        .unwrap();

        assert!(!provider.id.is_empty());
        assert_eq!(provider.provider_type, "anthropic");
        assert_eq!(provider.kind, "cli");
        assert_eq!(provider.name, "My Claude Code");
        assert_eq!(provider.default_model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(
            provider.binary_path.as_deref(),
            Some("/usr/local/bin/claude")
        );
        assert_eq!(provider.args_json.as_deref(), Some(r#"["--verbose"]"#));
        assert_eq!(
            provider.env_json.as_deref(),
            Some(r#"{"LOG_LEVEL":"info"}"#)
        );
        assert_eq!(provider.permission_mode.as_deref(), Some("accept-edits"));
        // config_json wrapper preserves `default_model` for the
        // service/sessions.rs:318 extractor.
        assert!(provider.config_json.contains("claude-sonnet-4-5"));
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

    /// feat-056: `list_for_runtime` is the resolution chokepoint for
    /// A2A `POST /api/a2a/messages`. Filters by the coarse
    /// `kind` column (`'http'` for HTTP runtimes, `'cli'` for CLI
    /// runtimes) — the per-runtime breakdown within HTTP/CLI is not
    /// carried by the schema today. Returns an empty `Vec` when no
    /// providers exist for the runtime — the caller raises
    /// `no_provider_for_runtime`. Creation order is preserved so the
    /// first created healthy provider wins, matching the
    /// "first provider in the list" semantics that the chokepoint
    /// replaces (but now scoped to the runtime, not the whole table).
    #[test]
    fn test_list_for_runtime_filters_by_kind() {
        let db = test_db();
        let config = sample_config();

        let http_first = ProviderStore::create(&db, "anthropic", "First HTTP", &config).unwrap();
        let _http_second = ProviderStore::create(&db, "anthropic", "Second HTTP", &config).unwrap();
        let http_third = ProviderStore::create(&db, "openai", "OpenAI HTTP", &config).unwrap();
        let cli_first = ProviderStore::create_cli(
            &db,
            "anthropic",
            "First CLI",
            "claude-sonnet-4-5",
            "/usr/local/bin/claude",
            "[]",
            "{}",
            "accept-edits",
        )
        .unwrap();
        let cli_second = ProviderStore::create_cli(
            &db,
            "anthropic",
            "Second CLI",
            "claude-sonnet-4-5",
            "/usr/local/bin/claude",
            "[]",
            "{}",
            "accept-edits",
        )
        .unwrap();

        // All HTTP runtimes → the three HTTP rows, in creation order.
        // Per-runtime distinction (AnthropicApi vs OpenaiApi) is
        // not carried by `kind='http'`; that's the table's
        // deliberate coarseness (see method docstring).
        let anthropic_rows =
            ProviderStore::list_for_runtime(&db, crate::agent::RuntimeKind::AnthropicApi).unwrap();
        assert_eq!(anthropic_rows.len(), 3);
        assert_eq!(anthropic_rows[0].id, http_first.id);
        assert_eq!(anthropic_rows[2].id, http_third.id);

        let openai_rows =
            ProviderStore::list_for_runtime(&db, crate::agent::RuntimeKind::OpenaiApi).unwrap();
        assert_eq!(openai_rows.len(), 3);
        assert_eq!(openai_rows[0].id, http_first.id);

        // All CLI runtimes → the two CLI rows, in creation order.
        let claude_rows =
            ProviderStore::list_for_runtime(&db, crate::agent::RuntimeKind::ClaudeCode).unwrap();
        assert_eq!(claude_rows.len(), 2);
        assert_eq!(claude_rows[0].id, cli_first.id);
        assert_eq!(claude_rows[1].id, cli_second.id);

        let codex_rows =
            ProviderStore::list_for_runtime(&db, crate::agent::RuntimeKind::Codex).unwrap();
        assert_eq!(codex_rows.len(), 2);
        assert_eq!(codex_rows[0].id, cli_first.id);
    }
}
