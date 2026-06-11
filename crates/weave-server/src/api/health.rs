use std::collections::BTreeMap;
use std::time::Instant;

use axum::Json;
use serde::Serialize;

use crate::AppState;

/// Shared server start time for computing uptime.
/// Set once during startup and passed into handlers via Extension.
#[derive(Clone, Copy)]
pub struct ServerStartTime(pub Instant);

/// Provider counts in the health response.
#[derive(Serialize)]
pub struct ProviderSummary {
    pub total: usize,
    pub healthy: usize,
    pub unhealthy: usize,
}

/// Database diagnostics in the health response.
///
/// `size_bytes` is `None` for `:memory:` databases. `reachable` reports
/// whether the synchronous probe (size + WAL checkpoint + count query)
/// completed without error.
#[derive(Serialize)]
pub struct DatabaseInfo {
    pub size_bytes: Option<u64>,
    pub wal_checkpoint_pending: bool,
    pub reachable: bool,
}

/// Enriched health response.
///
/// Returned raw (no `DataResponse` envelope) so that liveness probes
/// and external monitors see a stable, simple shape. The `status`
/// field is `"ok"` unless at least one of the following holds:
///
///   * No providers are healthy (`healthy == 0`), OR
///   * The database probe failed (`database.reachable == false`).
///
/// A fresh server with zero providers configured reports `"degraded"`
/// (literal reading of the rule). Operators wanting a different rule
/// for the empty-registry case can layer that logic in the operator's
/// monitoring system — the response still exposes the truth.
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub uptime_seconds: u64,
    pub providers: ProviderSummary,
    /// Per-workspace active session counts. Empty when there are no
    /// active sessions, or when the database probe failed.
    pub active_sessions: BTreeMap<String, u64>,
    pub database: DatabaseInfo,
}

/// GET /api/health
///
/// Returns server status, version, uptime, provider counts, per-workspace
/// active session counts, and database diagnostics. Always returns 200 —
/// health is a monitoring endpoint and should not 5xx on partial data;
/// the `status` field signals the aggregate verdict.
pub async fn health_check(
    axum::Extension(start_time): axum::Extension<ServerStartTime>,
    axum::Extension(state): axum::Extension<AppState>,
) -> Json<HealthResponse> {
    let uptime = start_time.0.elapsed().as_secs();

    // Provider health (parallel, 10s cached)
    let (total, healthy, unhealthy) = state.registry.cached_health_summary().await;

    // DB-derived fields. If the DB is unreachable, the response still
    // returns 200 with `database.reachable = false` and an empty
    // `active_sessions` map.
    let (active_sessions, database) = collect_db_info(&state.db);

    let degraded = healthy == 0 || !database.reachable;
    let status = if degraded { "degraded" } else { "ok" };

    Json(HealthResponse {
        status,
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: uptime,
        providers: ProviderSummary {
            total,
            healthy,
            unhealthy,
        },
        active_sessions,
        database,
    })
}

/// Run the synchronous DB probes (active-session aggregate, file size,
/// WAL checkpoint) and assemble the per-workspace map + `DatabaseInfo`.
///
/// Best-effort: a failure in any probe sets `reachable = false` and
/// leaves the per-workspace map empty. This matches the rest of the
/// codebase (handlers in `api/sessions.rs`, `api/workspaces.rs`) which
/// also call `db.conn()` directly from async contexts.
fn collect_db_info(db: &crate::db::Db) -> (BTreeMap<String, u64>, DatabaseInfo) {
    let size_bytes = db.size_bytes();
    let wal_checkpoint_pending = db.wal_checkpoint_pending();
    let active_sessions = crate::store::sessions::SessionStore::count_active_by_workspace(db);
    match active_sessions {
        Ok(map) => (
            map,
            DatabaseInfo {
                size_bytes,
                wal_checkpoint_pending,
                reachable: true,
            },
        ),
        Err(_) => (
            BTreeMap::new(),
            DatabaseInfo {
                size_bytes: None,
                wal_checkpoint_pending: false,
                reachable: false,
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::store::kanban_test_helpers::make_test_state;
    use crate::AppState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use serde_json::Value;
    use std::sync::Arc;
    use tower::ServiceExt;

    /// Build a test router around the enriched health endpoint.
    fn test_app(state: AppState) -> Router {
        let start = ServerStartTime(Instant::now());
        Router::new()
            .route("/api/health", get(health_check))
            .layer(axum::Extension(state))
            .layer(axum::Extension(start))
    }

    /// `test_health_check_detailed` — the verification command from
    /// `feature_list.json` for feat-033. Asserts the full enriched
    /// response shape against a fresh in-memory test state: no
    /// providers, no sessions, in-memory DB.
    #[tokio::test]
    async fn test_health_check_detailed() {
        let state = make_test_state();
        let app = test_app(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");

        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let v: Value = serde_json::from_slice(&body).expect("json");

        // Raw shape — no `DataResponse` envelope (health is a monitoring
        // endpoint; consumers expect raw JSON).
        assert!(v.get("data").is_none(), "health must be raw, not enveloped");

        assert_eq!(
            v["status"], "degraded",
            "empty registry has healthy==0, which triggers the degraded rule"
        );
        assert!(v["uptime_seconds"].as_u64().is_some());
        assert!(!v["version"].as_str().unwrap().is_empty());

        // providers section
        assert_eq!(v["providers"]["total"], 0);
        assert_eq!(v["providers"]["healthy"], 0);
        assert_eq!(v["providers"]["unhealthy"], 0);

        // active_sessions: empty BTreeMap → empty object
        assert_eq!(v["active_sessions"], serde_json::json!({}));

        // database: in-memory → null size, reachable, no pending WAL
        assert_eq!(v["database"]["size_bytes"], Value::Null);
        assert_eq!(v["database"]["wal_checkpoint_pending"], false);
        assert_eq!(v["database"]["reachable"], true);
    }

    /// `active_sessions` returns a per-workspace map keyed by
    /// `workspace_id`, with counts of non-terminal sessions.
    #[tokio::test]
    async fn test_health_active_sessions_per_workspace() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row(
                "SELECT id FROM workspaces WHERE name = 'default'",
                [],
                |r| r.get(0),
            )
            .expect("default workspace");
        let provider_id = crate::store::kanban_test_helpers::seed_provider(&state.db);

        // 2 active (one stays "connecting", one moves to "ready"),
        // 1 terminal (move to "completed")
        let _s_a = crate::store::sessions::SessionStore::create(
            &state.db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            crate::agent::RuntimeKind::default(),
            crate::agent::SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();
        let s_b = crate::store::sessions::SessionStore::create(
            &state.db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            crate::agent::RuntimeKind::default(),
            crate::agent::SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();
        let s_c = crate::store::sessions::SessionStore::create(
            &state.db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            crate::agent::RuntimeKind::default(),
            crate::agent::SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();
        crate::store::sessions::SessionStore::update_status(&state.db, &s_b.id, "ready").unwrap();
        crate::store::sessions::SessionStore::update_status(&state.db, &s_c.id, "completed")
            .unwrap();
        // s_a stays in its initial "connecting" state.

        let app = test_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let v: Value = serde_json::from_slice(&body).expect("json");

        assert_eq!(
            v["active_sessions"][ws_id].as_u64().unwrap(),
            2,
            "two active sessions (connecting + ready) in the default workspace"
        );
    }

    /// `database.size_bytes` is `Some(n)` for a file-backed DB and
    /// `None` for `:memory:`. Uses a dedicated `AppState` against a
    /// temp file so the in-memory default helper is not relied on.
    #[tokio::test]
    async fn test_health_database_size_bytes_for_file_db() {
        let path = std::env::temp_dir().join("weave-test-health-size.db");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));

        let db = Arc::new(Db::open(&path).expect("open file"));
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).expect("seed default");
        let state = AppState {
            db: db.clone(),
            registry: Arc::new(crate::agent::registry::ProviderRegistry::new()),
            active_sessions: Arc::new(crate::service::ActiveSessions::new()),
            active_child_processes: Arc::new(crate::service::ActiveChildProcesses::new()),
            sse_manager: Arc::new(crate::sse::SseManager::new()),
            specialists: Arc::new(crate::specialist::SpecialistRegistry::new()),
            tools: Arc::new(crate::tools::ToolRegistry::new()),
            a2a_token: None,
            shutdown_token: tokio_util::sync::CancellationToken::new(),
        };

        let app = test_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let v: Value = serde_json::from_slice(&body).expect("json");

        assert_eq!(v["database"]["reachable"], true);
        let size = v["database"]["size_bytes"].as_u64();
        assert!(size.is_some(), "file-backed DB has a measurable size");
        assert!(size.unwrap() > 0);

        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    /// A registry containing a healthy stub agent produces `status: "ok"`
    /// (the `else` branch of the degraded rule). Closes the only
    /// behavioral gap in the status logic.
    #[tokio::test]
    async fn test_health_check_status_ok_when_provider_healthy() {
        use crate::agent::registry::ProviderRegistry;
        use crate::agent::{CodingAgent, ModelInfo, ProviderHealth, StreamEvent};
        use crate::error::ProviderError;
        use async_trait::async_trait;
        use std::pin::Pin;

        struct HealthyStub;
        #[async_trait]
        impl CodingAgent for HealthyStub {
            fn provider_type(&self) -> &str {
                "stub"
            }
            fn display_name(&self) -> &str {
                "healthy-stub"
            }
            async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _req: crate::agent::MessageRequest,
                _turn: &crate::agent::turn_context::TurnContext,
            ) -> Result<
                Pin<
                    Box<dyn futures_util::Stream<Item = Result<StreamEvent, ProviderError>> + Send>,
                >,
                ProviderError,
            > {
                Err(ProviderError::Unreachable("stub".into()))
            }
            async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
                Ok(ProviderHealth {
                    healthy: true,
                    latency_ms: 1,
                    error: None,
                })
            }
        }

        let mut state = make_test_state();
        // Replace the empty registry with one containing a healthy stub.
        let registry = Arc::new(ProviderRegistry::new());
        registry.add_agent("p1", Arc::new(HealthyStub));
        state.registry = registry;

        let app = test_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let v: Value = serde_json::from_slice(&body).expect("json");

        assert_eq!(v["status"], "ok", "healthy provider + reachable DB → ok");
        assert_eq!(v["providers"]["total"], 1);
        assert_eq!(v["providers"]["healthy"], 1);
        assert_eq!(v["providers"]["unhealthy"], 0);
    }
}
