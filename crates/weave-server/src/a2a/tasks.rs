//! Task resource handlers — GetTask, CancelTask, SubscribeToTask.
//!
//! These handlers map A2A task operations to Weave session operations:
//! - GetTask → session status + message history
//! - CancelTask → cancel session
//! - SubscribeToTask → SSE stream of session events

use axum::extract::{Path, Query};
use axum::http::HeaderMap;
use axum::response::Sse;
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use std::convert::Infallible;
use tokio::sync::broadcast;

use super::auth::verify_a2a_token;
use super::types::*;
use crate::agent::StopReason;
use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::service::sessions::SessionService;
use crate::sse::SseWireEvent;
use crate::store::sessions::{MessageStore, SessionStore};
use crate::AppState;

// ---------------------------------------------------------------------------
// GetTask
// ---------------------------------------------------------------------------

/// `GET /api/a2a/tasks/{id}`
///
/// Authenticated. Returns the A2A Task for the given session ID,
/// including current status, optional message history, and artifacts.
pub async fn get_task(
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Query(params): Query<GetTaskParams>,
) -> Result<Json<DataResponse<Task>>, AppError> {
    verify_a2a_token(&state.a2a_token, &headers)?;

    let session = SessionStore::get_by_id(&state.db, &task_id)?;

    let history = if params.include_history.unwrap_or(false) {
        let messages = MessageStore::list_by_session(&state.db, &task_id, None, 100)?;
        Some(
            messages
                .data
                .iter()
                .map(|m| A2aMessage {
                    role: m.role.clone(),
                    parts: vec![Part::Text {
                        text: m.content.clone(),
                    }],
                })
                .collect(),
        )
    } else {
        None
    };

    let artifacts = if params.include_artifacts.unwrap_or(false) {
        Some(
            crate::store::artifacts::ArtifactStore::list_by_task(
                &state.db,
                &task_id,
                &session.workspace_id,
                None,
            )
            .unwrap_or_default()
            .into_iter()
            .map(|a| ArtifactRef {
                artifact_id: a.id,
                name: a.type_.clone(),
            })
            .collect(),
        )
    } else {
        None
    };

    let task = Task {
        id: session.id,
        context_id: session.context_id,
        status: TaskStatus::from_session_status(&session.status),
        history,
        artifacts,
    };

    Ok(Json(DataResponse { data: task }))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GetTaskParams {
    #[serde(default)]
    include_history: Option<bool>,
    #[serde(default)]
    include_artifacts: Option<bool>,
}

// ---------------------------------------------------------------------------
// CancelTask
// ---------------------------------------------------------------------------

/// `POST /api/a2a/tasks/{id}/cancel`
///
/// Authenticated. Cancels the A2A task (Weave session).
/// Idempotent — cancelling an already-cancelled task succeeds.
pub async fn cancel_task(
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Json<DataResponse<Task>>, AppError> {
    verify_a2a_token(&state.a2a_token, &headers)?;

    // Idempotent cancel — service returns Ok even if no active session
    SessionService::cancel_session(&state.active_sessions, &task_id)?;

    // Re-fetch to get updated status
    let session = SessionStore::get_by_id(&state.db, &task_id)?;

    let task = Task {
        id: session.id,
        context_id: session.context_id,
        status: TaskStatus::from_session_status(&session.status),
        history: None,
        artifacts: None,
    };

    Ok(Json(DataResponse { data: task }))
}

// ---------------------------------------------------------------------------
// SubscribeToTask (SSE)
// ---------------------------------------------------------------------------

/// SSE stream state for A2A task subscription.
enum SubscribeState {
    /// Emit the initial status event.
    Initial,
    /// Stream live events from the broadcast channel.
    Live,
    /// Stream has ended.
    Done,
}

/// `GET /api/a2a/tasks/{id}/subscribe`
///
/// Authenticated. Returns an SSE stream of A2A task events for the
/// given session. Emits `TaskStatusUpdate` events as the session
/// progresses, closing when the session reaches a terminal state.
///
/// Auth is checked before opening the SSE stream. If auth fails,
/// a 401 is returned immediately (not as an SSE event).
pub async fn subscribe_task(
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<
    Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, Infallible>>>,
    AppError,
> {
    verify_a2a_token(&state.a2a_token, &headers)?;

    // Validate session exists
    let _session = SessionStore::get_by_id(&state.db, &task_id)?;

    // Subscribe to the session's broadcast channel
    let rx = state.sse_manager.subscribe(&task_id);
    let db = state.db.clone();

    let stream = futures_util::stream::unfold(
        (SubscribeState::Initial, rx, task_id.clone(), db),
        |(mut state, mut rx, tid, db)| async move {
            loop {
                match state {
                    SubscribeState::Initial => {
                        state = SubscribeState::Live;
                        if let Ok(session) = SessionStore::get_by_id(&db, &tid) {
                            let initial_status = TaskStatus::from_session_status(&session.status);
                            let event = TaskEvent::TaskStatusUpdate {
                                task_id: tid.clone(),
                                status: initial_status,
                                message: None,
                            };
                            let json = serde_json::to_string(&event).unwrap_or_default();
                            let sse = axum::response::sse::Event::default()
                                .event("task_status_update")
                                .data(json);
                            return Some((Ok(sse), (state, rx, tid, db)));
                        }
                        // Session deleted between validation and now — stream nothing
                        return None;
                    }
                    SubscribeState::Live => {
                        match rx.recv().await {
                            Ok(event) => {
                                let (a2a_status, msg_opt) = map_sse_to_a2a(&event);
                                if let Some(status) = a2a_status {
                                    let is_terminal = matches!(
                                        status,
                                        TaskStatus::Completed
                                            | TaskStatus::Failed
                                            | TaskStatus::Canceled
                                    );
                                    if is_terminal {
                                        state = SubscribeState::Done;
                                    }
                                    let task_event = TaskEvent::TaskStatusUpdate {
                                        task_id: tid.clone(),
                                        status,
                                        message: msg_opt,
                                    };
                                    let json =
                                        serde_json::to_string(&task_event).unwrap_or_default();
                                    let sse = axum::response::sse::Event::default()
                                        .event("task_status_update")
                                        .data(json);
                                    return Some((Ok(sse), (state, rx, tid, db)));
                                }
                                // Non-status event — continue looping
                            }
                            Err(broadcast::error::RecvError::Closed) => return None,
                            Err(broadcast::error::RecvError::Lagged(_n)) => {
                                // Gap — emit current status as recovery
                                if let Ok(session) = SessionStore::get_by_id(&db, &tid) {
                                    let status = TaskStatus::from_session_status(&session.status);
                                    let task_event = TaskEvent::TaskStatusUpdate {
                                        task_id: tid.clone(),
                                        status,
                                        message: None,
                                    };
                                    let json =
                                        serde_json::to_string(&task_event).unwrap_or_default();
                                    let sse = axum::response::sse::Event::default()
                                        .event("task_status_update")
                                        .data(json);
                                    return Some((Ok(sse), (state, rx, tid, db)));
                                }
                                return None;
                            }
                        }
                    }
                    SubscribeState::Done => return None,
                }
            }
        },
    );

    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("heartbeat"),
    ))
}

// ---------------------------------------------------------------------------
// SSE event mapping
// ---------------------------------------------------------------------------

/// Map a Weave SSE wire event to an A2A task status + optional message.
///
/// Returns `None` for events that don't change the A2A task status
/// (e.g., raw thinking events, intermediate tool deltas).
fn map_sse_to_a2a(event: &SseWireEvent) -> (Option<TaskStatus>, Option<String>) {
    match event {
        SseWireEvent::TextDelta { .. } => (Some(TaskStatus::Working), None),
        SseWireEvent::Thinking { .. } => (Some(TaskStatus::Working), None),
        SseWireEvent::Done { stop_reason } => {
            let status = match stop_reason {
                StopReason::EndTurn | StopReason::MaxTokens | StopReason::ToolUse => {
                    TaskStatus::Completed
                }
                StopReason::Cancelled => TaskStatus::Canceled,
            };
            (Some(status), None)
        }
        SseWireEvent::Error { .. } => (Some(TaskStatus::Failed), None),
        _ => (None, None),
    }
}
