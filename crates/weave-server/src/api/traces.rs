use axum::extract::Path;
use axum::Json;

use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::store::traces::{FileChangeSummary, TraceRow, TraceStore};
use crate::AppState;

/// GET /api/sessions/:sid/trace
///
/// Returns all trace events for a session, ordered by timestamp.
pub async fn get_session_trace(
    axum::Extension(state): axum::Extension<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<DataResponse<Vec<TraceRow>>>, AppError> {
    let traces = TraceStore::list_by_session(&state.db, &session_id)?;
    Ok(Json(DataResponse { data: traces }))
}

/// GET /api/sessions/:sid/trace/journey
///
/// Returns journey events (decision + error) for a session, ordered by timestamp.
pub async fn get_session_journey(
    axum::Extension(state): axum::Extension<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<DataResponse<Vec<TraceRow>>>, AppError> {
    let journey = TraceStore::list_journey(&state.db, &session_id)?;
    Ok(Json(DataResponse { data: journey }))
}

/// GET /api/sessions/:sid/trace/tools
///
/// Returns tool_call events for a session, ordered by timestamp.
/// The Journey sidebar renders these as a third section ("Tools")
/// so a session that only used tools (e.g. list_notes, list_tasks)
/// doesn't render as empty.
pub async fn get_session_tool_calls(
    axum::Extension(state): axum::Extension<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<DataResponse<Vec<TraceRow>>>, AppError> {
    let tool_calls = TraceStore::list_tool_calls(&state.db, &session_id)?;
    Ok(Json(DataResponse { data: tool_calls }))
}

/// GET /api/sessions/:sid/trace/files
///
/// Returns deduplicated file changes for a session, grouped by path.
pub async fn get_session_file_changes(
    axum::Extension(state): axum::Extension<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<DataResponse<Vec<FileChangeSummary>>>, AppError> {
    let changes = TraceStore::list_file_changes(&state.db, &session_id)?;
    Ok(Json(DataResponse { data: changes }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::store::traces::{TraceEvent, TraceEventKind, TraceStore};

    /// Helper: build app state with a test DB that has seeded workspace + session.
    async fn test_app_with_traces(events: Vec<TraceEvent>) -> (axum::Router, String) {
        let db = crate::db::Db::open(std::path::Path::new(":memory:")).unwrap();

        // Seed workspace, provider, session (FK: sessions.provider_id -> providers.id)
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params!["ws-1", "test", "2026-01-01", "2026-01-01"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO providers (id, type, name, config_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["prov-1", "anthropic", "test", "{}", "2026-01-01"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO sessions (id, workspace_id, provider_id, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    "sess-1", "ws-1", "prov-1", "ready", "2026-01-01", "2026-01-01"
                ],
            )
            .unwrap();

        // Insert trace events
        if !events.is_empty() {
            TraceStore::insert_batch(&db, &events).unwrap();
        }

        let db = std::sync::Arc::new(db);
        let state = crate::AppState {
            db,
            registry: std::sync::Arc::new(crate::agent::registry::ProviderRegistry::new()),
            active_sessions: std::sync::Arc::new(crate::service::ActiveSessions::new()),
            sse_manager: std::sync::Arc::new(crate::sse::SseManager::new()),
            specialists: std::sync::Arc::new(crate::specialist::SpecialistRegistry::new()),
            tools: std::sync::Arc::new(crate::tools::ToolRegistry::new()),
            a2a_token: None,
            shutdown_token: tokio_util::sync::CancellationToken::new(),
        };

        let app = axum::Router::new()
            .route(
                "/api/sessions/{sid}/trace",
                axum::routing::get(super::get_session_trace),
            )
            .route(
                "/api/sessions/{sid}/trace/journey",
                axum::routing::get(super::get_session_journey),
            )
            .route(
                "/api/sessions/{sid}/trace/files",
                axum::routing::get(super::get_session_file_changes),
            )
            .route(
                "/api/sessions/{sid}/trace/tools",
                axum::routing::get(super::get_session_tool_calls),
            )
            .layer(axum::Extension(state));

        (app, "sess-1".to_string())
    }

    #[tokio::test]
    async fn test_get_session_trace() {
        let events = vec![
            TraceEvent {
                session_id: "sess-1".to_string(),
                kind: TraceEventKind::Decision {
                    text: "decided to use Rust".to_string(),
                },
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            TraceEvent {
                session_id: "sess-1".to_string(),
                kind: TraceEventKind::ToolCall {
                    tool_name: "fs_read".to_string(),
                    input_json: "{}".to_string(),
                    output_json: "{}".to_string(),
                    duration_ms: 10,
                },
                timestamp: "2026-01-01T00:00:01Z".to_string(),
            },
        ];

        let (app, _session_id) = test_app_with_traces(events).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/sess-1/trace")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let data = parsed["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["event_type"], "decision");
        assert_eq!(data[1]["event_type"], "tool_call");
    }

    #[tokio::test]
    async fn test_get_session_journey() {
        let events = vec![
            TraceEvent {
                session_id: "sess-1".to_string(),
                kind: TraceEventKind::Decision {
                    text: "chose approach A".to_string(),
                },
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            TraceEvent {
                session_id: "sess-1".to_string(),
                kind: TraceEventKind::ToolCall {
                    tool_name: "fs_read".to_string(),
                    input_json: "{}".to_string(),
                    output_json: "{}".to_string(),
                    duration_ms: 5,
                },
                timestamp: "2026-01-01T00:00:01Z".to_string(),
            },
            TraceEvent {
                session_id: "sess-1".to_string(),
                kind: TraceEventKind::Error {
                    message: "compilation failed".to_string(),
                },
                timestamp: "2026-01-01T00:00:02Z".to_string(),
            },
        ];

        let (app, _) = test_app_with_traces(events).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/sess-1/trace/journey")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let data = parsed["data"].as_array().unwrap();
        // Journey should have decision + error, but not tool_call
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["event_type"], "decision");
        assert_eq!(data[1]["event_type"], "error");
    }

    #[tokio::test]
    async fn test_get_session_tool_calls() {
        // Mixed events: the new endpoint should return only tool_call
        // events (not decision/error/file_change), ordered by timestamp.
        let events = vec![
            TraceEvent {
                session_id: "sess-1".to_string(),
                kind: TraceEventKind::Decision {
                    text: "ignored by tools endpoint".to_string(),
                },
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            TraceEvent {
                session_id: "sess-1".to_string(),
                kind: TraceEventKind::ToolCall {
                    tool_name: "list_notes".to_string(),
                    input_json: "{}".to_string(),
                    output_json: "{\"count\":0}".to_string(),
                    duration_ms: 3,
                },
                timestamp: "2026-01-01T00:00:01Z".to_string(),
            },
            TraceEvent {
                session_id: "sess-1".to_string(),
                kind: TraceEventKind::Error {
                    message: "also ignored".to_string(),
                },
                timestamp: "2026-01-01T00:00:02Z".to_string(),
            },
            TraceEvent {
                session_id: "sess-1".to_string(),
                kind: TraceEventKind::ToolCall {
                    tool_name: "list_tasks".to_string(),
                    input_json: "{}".to_string(),
                    output_json: "{\"count\":0}".to_string(),
                    duration_ms: 0,
                },
                timestamp: "2026-01-01T00:00:03Z".to_string(),
            },
        ];

        let (app, _) = test_app_with_traces(events).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/sess-1/trace/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let data = parsed["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["event_type"], "tool_call");
        assert_eq!(data[1]["event_type"], "tool_call");
        // The data_json is the Rust store's serialized form; the
        // frontend parses it and reads `data.tool_name`. Round-trip
        // through insert_batch so we exercise the same parse path
        // the journey sidebar uses.
        let first: serde_json::Value =
            serde_json::from_str(data[0]["data_json"].as_str().unwrap()).unwrap();
        assert_eq!(first["tool_name"], "list_notes");
    }

    #[tokio::test]
    async fn test_get_session_file_changes() {
        let events = vec![
            TraceEvent {
                session_id: "sess-1".to_string(),
                kind: crate::store::traces::TraceEventKind::FileChange {
                    path: "/tmp/a.rs".to_string(),
                    action: crate::store::traces::FileAction::Write,
                },
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            TraceEvent {
                session_id: "sess-1".to_string(),
                kind: crate::store::traces::TraceEventKind::FileChange {
                    path: "/tmp/a.rs".to_string(),
                    action: crate::store::traces::FileAction::Write,
                },
                timestamp: "2026-01-01T00:00:01Z".to_string(),
            },
        ];

        let (app, _) = test_app_with_traces(events).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/sess-1/trace/files")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let data = parsed["data"].as_array().unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["path"], "/tmp/a.rs");
        assert_eq!(data[0]["count"], 2);
    }
}
