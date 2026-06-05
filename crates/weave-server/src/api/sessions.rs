use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::stream;
use serde::Deserialize;
use std::convert::Infallible;

use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::service::sessions::SessionService;
use crate::sse::SseWireEvent;
use crate::store::sessions::{MessagePage, MessageStore, SessionPage, SessionStore};
use crate::AppState;

const DEFAULT_PAGE_LIMIT: u32 = 100;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListParams {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

impl ListParams {
    pub fn effective_limit(&self) -> u32 {
        self.limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, 100)
    }
}

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub provider_id: String,
    pub specialist_id: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub parent_session_id: Option<String>,
    /// Optional codebase binding. When set, the codebase must belong to
    /// the same workspace, and the codebase's `path` is copied onto the
    /// session's `cwd` (overriding any supplied `cwd`).
    pub codebase_id: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateStatusRequest {
    pub status: String,
}

#[derive(Deserialize)]
pub struct PromptRequest {
    pub prompt: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/workspaces/{wid}/sessions
pub async fn create_session(
    axum::Extension(state): axum::Extension<AppState>,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateSessionRequest>,
) -> Result<
    (
        StatusCode,
        Json<DataResponse<crate::store::sessions::Session>>,
    ),
    AppError,
> {
    let session = SessionService::create_session(
        &state.db,
        &workspace_id,
        &body.provider_id,
        body.specialist_id.as_deref(),
        body.model.as_deref(),
        body.cwd.as_deref(),
        body.parent_session_id.as_deref(),
        None, // context_id — not set via the standard session API
        body.codebase_id.as_deref(),
    )?;

    Ok((StatusCode::CREATED, Json(DataResponse { data: session })))
}

/// GET /api/workspaces/{wid}/sessions
pub async fn list_sessions(
    axum::Extension(state): axum::Extension<AppState>,
    Path(workspace_id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<DataResponse<SessionPage>>, AppError> {
    let page = SessionStore::list_by_workspace(
        &state.db,
        &workspace_id,
        params.cursor.as_deref(),
        params.effective_limit(),
    )?;
    Ok(Json(DataResponse { data: page }))
}

/// GET /api/sessions/{id}
pub async fn get_session(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<crate::store::sessions::Session>>, AppError> {
    let session = SessionStore::get_by_id(&state.db, &id)?;
    Ok(Json(DataResponse { data: session }))
}

/// DELETE /api/sessions/{id}
pub async fn delete_session(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<()>>, AppError> {
    SessionStore::delete(&state.db, &id)?;
    Ok(Json(DataResponse { data: () }))
}

/// PATCH /api/sessions/{id}/status
pub async fn update_session_status(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateStatusRequest>,
) -> Result<Json<DataResponse<crate::store::sessions::Session>>, AppError> {
    let session = SessionStore::update_status(&state.db, &id, &body.status)?;
    Ok(Json(DataResponse { data: session }))
}

/// GET /api/sessions/{sid}/history
pub async fn get_session_history(
    axum::Extension(state): axum::Extension<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<DataResponse<MessagePage>>, AppError> {
    let page = MessageStore::list_by_session(
        &state.db,
        &session_id,
        params.cursor.as_deref(),
        params.effective_limit(),
    )?;
    Ok(Json(DataResponse { data: page }))
}

/// POST /api/sessions/{sid}/prompt
///
/// Returns `{"data": {"message_id": "..."}}` immediately. Spawns an async
/// task that streams the agent response and saves the assistant message.
pub async fn send_prompt(
    axum::Extension(state): axum::Extension<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<PromptRequest>,
) -> Result<(StatusCode, Json<DataResponse<serde_json::Value>>), AppError> {
    let message_id = crate::service::sessions::SessionService::send_prompt(
        &state.db,
        &state.registry,
        &state.specialists,
        &state.active_sessions,
        &state.sse_manager,
        &state.tools,
        &session_id,
        &body.prompt,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(DataResponse {
            data: serde_json::json!({ "message_id": message_id }),
        }),
    ))
}

/// GET /api/sessions/{sid}/stream
///
/// Returns an SSE stream for the session. Supports `Last-Event-ID` header
/// for reconnection replay. Events are: connected, text_delta,
/// tool_use_start, tool_use_delta, tool_result, thinking, done, error, gap.
/// Heartbeat comments are sent every 15s via `keep_alive`.
pub async fn session_stream(
    axum::Extension(state): axum::Extension<AppState>,
    Path(session_id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    // Verify session exists — send error event and close if not found.
    // We can't return AppError here because Sse requires a Stream, not a Result.
    let session_exists =
        crate::store::sessions::SessionStore::get_by_id(&state.db, &session_id).is_ok();

    let last_event_id: Option<u64> = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let sse_manager = state.sse_manager.clone();
    let sid = session_id.clone();

    // Strategy: subscribe first (so we don't miss events), then read buffer.
    // Events that arrive between subscribe and buffer read will be in both
    // the buffer and the receiver. We deduplicate by tracking the max buffered
    // ID and skipping receiver events with ID <= max_buffered_id in Live state.
    let sse_stream = stream::unfold(
        (
            sse_manager,
            sid,
            last_event_id,
            SseState::Initial,
            session_exists,
        ),
        |(mgr, sid, last_id, state, session_exists)| async move {
            match state {
                SseState::Initial => {
                    // If session doesn't exist, send error event and close
                    if !session_exists {
                        let error_event = make_sse_event(
                            None,
                            "error",
                            &serde_json::to_string(&SseWireEvent::Error {
                                message: "session not found".into(),
                            })
                            .unwrap_or_default(),
                        );
                        return Some((Ok(error_event), (mgr, sid, last_id, SseState::Done, false)));
                    }

                    // Subscribe first to avoid missing events
                    let rx = mgr.subscribe(&sid);

                    // Send connected event (no ID — protocol event, not part of replay)
                    let connected = make_sse_event(
                        None,
                        "connected",
                        &crate::sse::sse_data(&SseWireEvent::Connected {
                            session_id: sid.clone(),
                        }),
                    );

                    // Read buffered events for replay. The ring buffer holds
                    // the last 100 events for the session, which spans
                    // multiple turns. Replaying ALL of them on a fresh
                    // connection (e.g. page reload) would cause the
                    // frontend to re-apply text_deltas from prior turns
                    // to its live buffer, producing duplicate / stale
                    // bubbles. The browser's EventSource only sends a
                    // `Last-Event-ID` header on auto-reconnects, NOT on
                    // the first connection — so we treat a missing
                    // `last_id` as a fresh mount and skip the replay
                    // entirely. Mid-turn reconnects within a session
                    // will still get the relevant buffered events.
                    let buffered: Vec<_> = match last_id {
                        Some(after_id) => mgr.get_after(&sid, after_id),
                        None => Vec::new(),
                    };
                    let max_id = buffered.last().map(|e| e.id).unwrap_or(0);

                    // Check for gap (reconnection with missing events)
                    if let Some(after_id) = last_id {
                        let expected_next = after_id + 1;
                        let first_buffered = buffered.first().map(|e| e.id).unwrap_or(0);
                        if !buffered.is_empty() && first_buffered > expected_next {
                            let gap = make_sse_event(
                                None,
                                "gap",
                                &serde_json::to_string(&SseWireEvent::Gap {
                                    missed: first_buffered - expected_next,
                                })
                                .unwrap_or_default(),
                            );
                            return Some((
                                Ok(connected),
                                (
                                    mgr,
                                    sid,
                                    last_id,
                                    SseState::Gap(gap, buffered, 0, rx, max_id),
                                    true,
                                ),
                            ));
                        }
                    }

                    if !buffered.is_empty() {
                        Some((
                            Ok(connected),
                            (
                                mgr,
                                sid,
                                last_id,
                                SseState::Buffered(buffered, 0, rx, max_id),
                                true,
                            ),
                        ))
                    } else {
                        Some((
                            Ok(connected),
                            (mgr, sid, last_id, SseState::Live(rx, max_id), true),
                        ))
                    }
                }
                SseState::Gap(gap_event, buffered, idx, rx, max_id) => Some((
                    Ok(gap_event),
                    (
                        mgr,
                        sid,
                        last_id,
                        SseState::Buffered(buffered, idx, rx, max_id),
                        true,
                    ),
                )),
                SseState::Buffered(buffered, idx, rx, max_id) => {
                    if idx < buffered.len() {
                        let entry = &buffered[idx];
                        let event = make_sse_event(Some(entry.id), &entry.event_type, &entry.data);
                        Some((
                            Ok(event),
                            (
                                mgr,
                                sid,
                                last_id,
                                SseState::Buffered(buffered, idx + 1, rx, max_id),
                                true,
                            ),
                        ))
                    } else {
                        // Buffered events done — transition to live with dedup threshold
                        Some((
                            Ok(make_sse_event(None, "buffered_complete", "{}")),
                            (mgr, sid, last_id, SseState::Live(rx, max_id), true),
                        ))
                    }
                }
                SseState::Live(mut rx, max_buffered_id) => {
                    // Loop to skip events already sent from buffer (deduplication)
                    loop {
                        match rx.recv().await {
                            Ok(wire_event) => {
                                // Get the event ID — it was the previous broadcast,
                                // so its ID is current_id - 1
                                let current_id = mgr.get_current_id(&sid);
                                let event_id = current_id.saturating_sub(1);

                                // Deduplicate: skip events already replayed from buffer
                                if event_id <= max_buffered_id {
                                    continue;
                                }

                                let event_type = wire_event.event_type().to_string();
                                let data = crate::sse::sse_data(&wire_event);
                                let event = make_sse_event(Some(event_id), &event_type, &data);
                                break Some((
                                    Ok(event),
                                    (mgr, sid, last_id, SseState::Live(rx, max_buffered_id), true),
                                ));
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                let gap = make_sse_event(
                                    None,
                                    "gap",
                                    &serde_json::to_string(&SseWireEvent::Gap { missed: n })
                                        .unwrap_or_default(),
                                );
                                break Some((
                                    Ok(gap),
                                    (mgr, sid, last_id, SseState::Live(rx, max_buffered_id), true),
                                ));
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break None,
                        }
                    }
                }
                SseState::Done => None,
            }
        },
    );

    Sse::new(sse_stream).keep_alive(KeepAlive::default())
}

/// SSE stream state machine for event replay and live streaming.
///
/// The `max_id` field (present on `Gap`, `Buffered`, and `Live`) tracks the
/// highest buffered event ID. When transitioning to `Live`, events from the
/// broadcast receiver with ID <= max_id are skipped (they were already replayed
/// from the buffer).
enum SseState {
    /// Initial state — subscribe, send connected, determine replay path
    Initial,
    /// Sending a gap event before buffered replay
    Gap(
        Event,
        Vec<crate::sse::BufferedEvent>,
        usize,
        tokio::sync::broadcast::Receiver<SseWireEvent>,
        u64,
    ),
    /// Replaying buffered events
    Buffered(
        Vec<crate::sse::BufferedEvent>,
        usize,
        tokio::sync::broadcast::Receiver<SseWireEvent>,
        u64,
    ),
    /// Streaming live events (with deduplication threshold)
    Live(tokio::sync::broadcast::Receiver<SseWireEvent>, u64),
    /// Terminal state — stream ended
    Done,
}

/// Helper to construct an SSE `Event` with optional ID, event type, and data.
fn make_sse_event(id: Option<u64>, event_type: &str, data: &str) -> Event {
    let mut event = Event::default().event(event_type).data(data);
    if let Some(id) = id {
        event = event.id(id.to_string());
    }
    event
}

/// POST /api/sessions/{sid}/cancel
///
/// Cancels an active streaming task for the session.
pub async fn cancel_session(
    axum::Extension(state): axum::Extension<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<DataResponse<serde_json::Value>>, AppError> {
    crate::service::sessions::SessionService::cancel_session(&state.active_sessions, &session_id)?;

    Ok(Json(DataResponse {
        data: serde_json::json!({ "status": "cancelled" }),
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use serde_json::Value;
    use std::path::Path;
    use tower::ServiceExt;

    fn test_app() -> (Router, String, String) {
        let db = std::sync::Arc::new(Db::open(Path::new(":memory:")).unwrap());
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = std::sync::Arc::new(crate::agent::registry::ProviderRegistry::new());
        let active_sessions = std::sync::Arc::new(crate::service::ActiveSessions::new());
        let sse_manager = std::sync::Arc::new(crate::sse::SseManager::new());
        let specialists = std::sync::Arc::new(crate::specialist::SpecialistRegistry::new());
        let tools = std::sync::Arc::new(crate::tools::ToolRegistry::new());
        let state = AppState {
            db: db.clone(),
            registry,
            active_sessions,
            sse_manager,
            specialists,
            tools,
            a2a_token: None,
            shutdown_token: tokio_util::sync::CancellationToken::new(),
        };
        let start_time = crate::api::health::ServerStartTime(std::time::Instant::now());

        let (ws_id, provider_id) = crate::store::sessions::tests::seed_deps(&db);

        let router = Router::new()
            .route(
                "/api/workspaces/{wid}/sessions",
                axum::routing::get(list_sessions).post(create_session),
            )
            .route(
                "/api/sessions/{id}",
                axum::routing::get(get_session)
                    .patch(update_session_status)
                    .delete(delete_session),
            )
            .route(
                "/api/sessions/{sid}/history",
                axum::routing::get(get_session_history),
            )
            .route(
                "/api/sessions/{sid}/prompt",
                axum::routing::post(send_prompt),
            )
            .route(
                "/api/sessions/{sid}/cancel",
                axum::routing::post(cancel_session),
            )
            .route(
                "/api/sessions/{sid}/stream",
                axum::routing::get(session_stream),
            )
            .layer(axum::Extension(state))
            .layer(axum::Extension(start_time));

        (router, ws_id, provider_id)
    }

    fn extract_json(body: &[u8]) -> Value {
        serde_json::from_slice(body).unwrap()
    }

    #[tokio::test]
    async fn test_session_lifecycle() {
        let (app, ws_id, provider_id) = test_app();

        // CREATE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/sessions", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"provider_id":"{}","model":"claude-sonnet-4-20250514","cwd":"/tmp"}}"#,
                        provider_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let session = &json["data"];
        assert_eq!(session["workspace_id"], ws_id);
        assert_eq!(session["provider_id"], provider_id);
        assert_eq!(session["status"], "connecting");
        assert_eq!(session["model"], "claude-sonnet-4-20250514");
        assert_eq!(session["cwd"], "/tmp");
        let session_id = session["id"].as_str().unwrap().to_string();

        // GET
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{}", session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert_eq!(json["data"]["id"], session_id);

        // UPDATE STATUS
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{}", session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"status":"ready"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert_eq!(json["data"]["status"], "ready");

        // DELETE
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/sessions/{}", session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify deleted
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{}", session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_session_state_transitions() {
        let (app, ws_id, provider_id) = test_app();

        // Create session
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/sessions", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"provider_id":"{}"}}"#,
                        provider_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let session_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // connecting -> ready (valid)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{}", session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"status":"ready"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // ready -> completed (valid)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{}", session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"status":"completed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // completed -> ready (invalid — terminal)
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{}", session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"status":"ready"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_session_not_found() {
        let (app, _, _) = test_app();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_session_invalid_provider() {
        let (app, ws_id, _) = test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/sessions", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"provider_id":"nonexistent"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_session_list_empty() {
        let (app, ws_id, _) = test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/sessions", ws_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = extract_json(&body);
        let items = json["data"]["data"].as_array().unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_send_prompt_empty_body() {
        let (app, ws_id, provider_id) = test_app();

        // Create a session first
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/sessions", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"provider_id":"{}"}}"#,
                        provider_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let session_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Send empty prompt — should fail
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{}/prompt", session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"prompt":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_send_prompt_session_not_found() {
        let (app, _, _) = test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/nonexistent/prompt")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"prompt":"hello"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_cancel_session_not_active() {
        let (app, ws_id, provider_id) = test_app();

        // Create a session
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/sessions", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"provider_id":"{}"}}"#,
                        provider_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let session_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Cancel without active prompt — should fail
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{}/cancel", session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
