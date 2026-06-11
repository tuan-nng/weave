//! HTTP API for kanban: boards, columns, tasks.
//!
//! Route layout (registered in `api/mod.rs`):
//! - `POST   /api/workspaces/{wid}/boards`              create_board
//! - `GET    /api/workspaces/{wid}/boards`              list_boards
//! - `GET    /api/workspaces/{wid}/boards/{id}`         get_board  (composite)
//! - `PATCH  /api/workspaces/{wid}/boards/{id}`         update_board
//! - `DELETE /api/workspaces/{wid}/boards/{id}`         delete_board
//! - `POST   /api/workspaces/{wid}/boards/{bid}/columns` create_column
//! - `PATCH  /api/columns/{cid}`                        update_column
//! - `POST   /api/workspaces/{wid}/boards/{bid}/cards`  create_card
//! - `PATCH  /api/tasks/{tid}`                          update_task
//! - `DELETE /api/tasks/{tid}`                          delete_task
//! - `GET    /api/boards/{bid}/stream`                  board_stream (SSE)
//!
//! All board-scoped routes take `wid` in the URL and the handler
//! verifies the board's `workspace_id` matches before proceeding.
//! This prevents cross-workspace data access by guessed UUID.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::Json;
use futures_util::stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::time::Duration;

use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::service::kanban::try_automate_lane;
use crate::sse::SseWireEvent;
use crate::store::boards::{BoardStore, NewColumnSpec};
use crate::store::columns::{Column, ColumnStore};
use crate::store::providers::ProviderStore;
use crate::store::tasks::{Task, TaskStore, UpdateTask};
use crate::store::workspaces::WorkspaceStore;
use crate::AppState;

const MAX_BOARD_NAME_LEN: usize = 100;
const MAX_COLUMN_NAME_LEN: usize = 100;
const MAX_TASK_TITLE_LEN: usize = 500;

// ---------------------------------------------------------------------------
// Request DTOs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateBoardRequest {
    pub name: String,
    pub columns: Option<Vec<CreateColumnInline>>,
}

#[derive(Deserialize)]
pub struct CreateColumnInline {
    pub name: String,
    pub position: Option<i64>,
    pub specialist_id: Option<String>,
    pub auto_trigger: Option<bool>,
    // Transition-gate fields (feat-028, migration 006). The HTTP API does
    // not yet expose these to clients — defaults to `None` / no-op. A
    // future feature can add setters without changing the DTO shape.
    pub freeze_description: Option<bool>,
    pub required_fields: Option<Vec<String>>,
    pub required_artifact_types: Option<Vec<String>>,
    /// Nullable Runtime Tool binding for lane automation (feat-055).
    pub runtime_kind: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateBoardRequest {
    pub name: String,
}

#[derive(Deserialize)]
pub struct CreateColumnRequest {
    pub name: String,
    pub position: Option<i64>,
    pub specialist_id: Option<String>,
    pub auto_trigger: Option<bool>,
    pub freeze_description: Option<bool>,
    pub required_fields: Option<Vec<String>>,
    pub required_artifact_types: Option<Vec<String>>,
    /// Nullable Runtime Tool binding for lane automation (feat-055).
    pub runtime_kind: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateColumnRequest {
    pub name: Option<String>,
    pub position: Option<i64>,
    /// `Some(None)` clears the specialist binding. `None` leaves it unchanged.
    pub specialist_id: Option<Option<String>>,
    pub auto_trigger: Option<bool>,
    pub freeze_description: Option<bool>,
    pub required_fields: Option<Vec<String>>,
    pub required_artifact_types: Option<Vec<String>>,
    /// `Some(None)` clears the runtime_kind binding. `None` leaves it unchanged.
    pub runtime_kind: Option<Option<String>>,
}

#[derive(Deserialize)]
pub struct CreateCardRequest {
    pub column_id: String,
    pub title: String,
    pub description: Option<String>,
    pub position: Option<i64>,
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateTaskRequest {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub column_id: Option<String>,
    pub position: Option<i64>,
    pub status: Option<String>,
    pub session_id: Option<Option<String>>,
    pub acceptance_criteria: Option<Option<String>>,
    pub completion_summary: Option<Option<String>>,
    pub verification_report: Option<Option<String>>,
}

impl From<UpdateTaskRequest> for UpdateTask {
    fn from(r: UpdateTaskRequest) -> Self {
        UpdateTask {
            title: r.title,
            description: r.description,
            column_id: r.column_id,
            position: r.position,
            status: r.status,
            session_id: r.session_id,
            acceptance_criteria: r.acceptance_criteria,
            completion_summary: r.completion_summary,
            verification_report: r.verification_report,
        }
    }
}

// ---------------------------------------------------------------------------
// Board handlers
// ---------------------------------------------------------------------------

/// POST /api/workspaces/{wid}/boards
pub async fn create_board(
    axum::Extension(state): axum::Extension<AppState>,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateBoardRequest>,
) -> Result<(StatusCode, Json<DataResponse<crate::store::boards::Board>>), AppError> {
    let name = body.name.trim();
    validate_board_name(name)?;

    let template: Vec<NewColumnSpec<'_>> = body
        .columns
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|c| {
            let trimmed = c.name.trim();
            if trimmed.is_empty() {
                return Err(AppError::validation("column name must not be empty"));
            }
            if trimmed.chars().count() > MAX_COLUMN_NAME_LEN {
                return Err(AppError::validation(format!(
                    "column name must be at most {} characters",
                    MAX_COLUMN_NAME_LEN
                )));
            }
            Ok(NewColumnSpec {
                name: trimmed,
                position: c.position,
                specialist_id: c.specialist_id.as_deref(),
                auto_trigger: c.auto_trigger.unwrap_or(false),
                freeze_description: c.freeze_description.unwrap_or(false),
                required_fields: c.required_fields.clone().unwrap_or_default(),
                required_artifact_types: c.required_artifact_types.clone().unwrap_or_default(),
                runtime_kind: c.runtime_kind.as_deref(),
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    let board = BoardStore::create(&state.db, &workspace_id, name, &template)?;
    Ok((StatusCode::CREATED, Json(DataResponse { data: board })))
}

/// GET /api/workspaces/{wid}/boards
pub async fn list_boards(
    axum::Extension(state): axum::Extension<AppState>,
    Path(workspace_id): Path<String>,
) -> Result<Json<DataResponse<Vec<crate::store::boards::Board>>>, AppError> {
    let boards = BoardStore::list_by_workspace(&state.db, &workspace_id)?;
    Ok(Json(DataResponse { data: boards }))
}

// ---------------------------------------------------------------------------
// Unbound-tasks handler (feat-053) — surfaces "active tasks not currently
// being worked on" for the new-session wizard's Step 4.
// ---------------------------------------------------------------------------

/// Query string for `GET /api/workspaces/{wid}/tasks`.
///
/// The `unbound` parameter is REQUIRED. `?unbound=true` returns active
/// tasks without a `session_id`; `?unbound=false` (or absent) is
/// rejected with 400 — the endpoint exists for one specific query and
/// the wizard always wants unbound. Future filter needs (e.g. by
/// `column_id`, by `tag`) can grow this DTO without a path change.
#[derive(Deserialize)]
pub struct ListUnboundTasksQuery {
    pub unbound: Option<bool>,
}

/// GET /api/workspaces/{wid}/tasks?unbound=true
///
/// Returns the workspace's active tasks that have no session bound
/// (`status = 'active' AND session_id IS NULL`). Used by the
/// new-session wizard's Step 4 (feat-053) to let the user attach a
/// session to an existing backlog card.
///
/// The endpoint exists for one purpose; the strict `?unbound=true`
/// filter keeps the URL grammar deterministic — a `?unbound=false`
/// query is meaningless (it would be `list` with a different name) so
/// we reject it explicitly rather than silently returning all tasks.
pub async fn list_unbound_tasks(
    axum::Extension(state): axum::Extension<AppState>,
    Path(workspace_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<ListUnboundTasksQuery>,
) -> Result<Json<DataResponse<Vec<Task>>>, AppError> {
    if params.unbound != Some(true) {
        return Err(AppError::validation_with_code(
            "missing_unbound_filter",
            "this endpoint requires ?unbound=true",
        ));
    }
    // Explicit 404 on unknown workspace, matching the parity of
    // `GET /api/workspaces/{wid}` (which 404s on unknown id) and
    // `get_board`'s cross-workspace guard. The store's JOIN would
    // also return an empty Vec for an unknown wid, but a silent
    // empty array is worse for the wizard UX (it would render "0
    // tasks" for a typo'd workspace id).
    if WorkspaceStore::get_by_id(&state.db, &workspace_id).is_err() {
        return Err(AppError::NotFound {
            resource: "workspace".into(),
            id: workspace_id,
        });
    }
    let tasks = TaskStore::list_unbound_in_workspace(&state.db, &workspace_id)?;
    Ok(Json(DataResponse { data: tasks }))
}

/// GET /api/workspaces/{wid}/boards/{id}
///
/// Composite response: board + columns + tasks.
pub async fn get_board(
    axum::Extension(state): axum::Extension<AppState>,
    Path((workspace_id, id)): Path<(String, String)>,
) -> Result<Json<DataResponse<crate::store::boards::BoardDetail>>, AppError> {
    let detail = BoardStore::get_with_children(&state.db, &id)?;
    if detail.board.workspace_id != workspace_id {
        return Err(AppError::NotFound {
            resource: "board".into(),
            id,
        });
    }
    Ok(Json(DataResponse { data: detail }))
}

/// PATCH /api/workspaces/{wid}/boards/{id}
pub async fn update_board(
    axum::Extension(state): axum::Extension<AppState>,
    Path((workspace_id, id)): Path<(String, String)>,
    Json(body): Json<UpdateBoardRequest>,
) -> Result<Json<DataResponse<crate::store::boards::Board>>, AppError> {
    let name = body.name.trim();
    validate_board_name(name)?;
    // Verify the board belongs to the requesting workspace before mutating.
    let board = BoardStore::get_by_id(&state.db, &id)?;
    if board.workspace_id != workspace_id {
        return Err(AppError::NotFound {
            resource: "board".into(),
            id,
        });
    }
    let board = BoardStore::update_name(&state.db, &id, name)?;
    Ok(Json(DataResponse { data: board }))
}

/// DELETE /api/workspaces/{wid}/boards/{id}
pub async fn delete_board(
    axum::Extension(state): axum::Extension<AppState>,
    Path((workspace_id, id)): Path<(String, String)>,
) -> Result<Json<DataResponse<()>>, AppError> {
    let board = BoardStore::get_by_id(&state.db, &id)?;
    if board.workspace_id != workspace_id {
        return Err(AppError::NotFound {
            resource: "board".into(),
            id,
        });
    }
    BoardStore::delete(&state.db, &id)?;
    Ok(Json(DataResponse { data: () }))
}

// ---------------------------------------------------------------------------
// Column handlers
// ---------------------------------------------------------------------------

/// POST /api/workspaces/{wid}/boards/{bid}/columns
pub async fn create_column(
    axum::Extension(state): axum::Extension<AppState>,
    Path((workspace_id, board_id)): Path<(String, String)>,
    Json(body): Json<CreateColumnRequest>,
) -> Result<
    (
        StatusCode,
        Json<DataResponse<crate::store::columns::Column>>,
    ),
    AppError,
> {
    // Verify the board belongs to the requesting workspace.
    let board = BoardStore::get_by_id(&state.db, &board_id)?;
    if board.workspace_id != workspace_id {
        return Err(AppError::NotFound {
            resource: "board".into(),
            id: board_id,
        });
    }
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::validation("column name must not be empty"));
    }
    if name.chars().count() > MAX_COLUMN_NAME_LEN {
        return Err(AppError::validation(format!(
            "column name must be at most {} characters",
            MAX_COLUMN_NAME_LEN
        )));
    }
    let column = ColumnStore::create(
        &state.db,
        &board_id,
        name,
        body.position,
        body.specialist_id.as_deref(),
        body.auto_trigger.unwrap_or(false),
        body.freeze_description,
        body.required_fields.as_deref(),
        body.required_artifact_types.as_deref(),
        body.runtime_kind.as_deref(),
    )?;
    let column_json = serde_json::to_value(&column).map_err(|e| {
        AppError::Internal(anyhow::anyhow!("failed to serialize column for SSE: {e}"))
    })?;
    state.sse_manager.broadcast(
        &format!("board:{}", board_id),
        SseWireEvent::ColumnAdded {
            column: column_json,
        },
    );
    Ok((StatusCode::CREATED, Json(DataResponse { data: column })))
}

/// PATCH /api/columns/{cid}
pub async fn update_column(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateColumnRequest>,
) -> Result<Json<DataResponse<crate::store::columns::Column>>, AppError> {
    let name = body.name.as_deref().map(str::trim);
    if let Some(n) = name {
        if n.is_empty() {
            return Err(AppError::validation("column name must not be empty"));
        }
        if n.chars().count() > MAX_COLUMN_NAME_LEN {
            return Err(AppError::validation(format!(
                "column name must be at most {} characters",
                MAX_COLUMN_NAME_LEN
            )));
        }
    }
    let column = ColumnStore::update(
        &state.db,
        &id,
        name,
        body.position,
        body.specialist_id.as_ref().map(|s| s.as_deref()),
        body.auto_trigger,
        body.freeze_description,
        body.required_fields.as_deref(),
        body.required_artifact_types.as_deref(),
        body.runtime_kind.as_ref().map(|s| s.as_deref()),
    )?;
    Ok(Json(DataResponse { data: column }))
}

// ---------------------------------------------------------------------------
// Task handlers
// ---------------------------------------------------------------------------

/// POST /api/workspaces/{wid}/boards/{bid}/cards
pub async fn create_card(
    axum::Extension(state): axum::Extension<AppState>,
    Path((workspace_id, board_id)): Path<(String, String)>,
    Json(body): Json<CreateCardRequest>,
) -> Result<(StatusCode, Json<DataResponse<Task>>), AppError> {
    // Verify the board belongs to the requesting workspace.
    let board = BoardStore::get_by_id(&state.db, &board_id)?;
    if board.workspace_id != workspace_id {
        return Err(AppError::NotFound {
            resource: "board".into(),
            id: board_id,
        });
    }
    let title = body.title.trim();
    if title.is_empty() {
        return Err(AppError::validation("task title must not be empty"));
    }
    if title.chars().count() > MAX_TASK_TITLE_LEN {
        return Err(AppError::validation(format!(
            "task title must be at most {} characters",
            MAX_TASK_TITLE_LEN
        )));
    }
    let task = TaskStore::create(
        &state.db,
        &board_id,
        &body.column_id,
        title,
        body.description.as_deref(),
        body.position,
        body.status.as_deref(),
    )?;
    let task_json = serde_json::to_value(&task).map_err(|e| {
        AppError::Internal(anyhow::anyhow!("failed to serialize task for SSE: {e}"))
    })?;
    state.sse_manager.broadcast(
        &format!("board:{}", board_id),
        SseWireEvent::TaskCreated { task: task_json },
    );
    Ok((StatusCode::CREATED, Json(DataResponse { data: task })))
}

/// PATCH /api/tasks/{tid}
///
/// Supports any subset of the editable fields. Use `column_id` to
/// move a task between columns; the move triggers an automatic
/// position rebalance in the target column, then runs lane automation
/// if the destination column has `auto_trigger=true`.
pub async fn update_task(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskRequest>,
) -> Result<Json<DataResponse<Task>>, AppError> {
    let fields = UpdateTask::from(body);
    let column_changed = fields.column_id.is_some();
    let old_column_id = if column_changed {
        // Capture the pre-move column so we can broadcast `task_moved`
        // with the right `from_column_id`. The task lookup is one extra
        // SQL hit per move, but moves are infrequent.
        Some(lookup_task_column_id(&state.db, &id)?)
    } else {
        None
    };
    // Cache the destination column for the auto-trigger path. We fetch
    // it once and reuse for the precheck AND the post-move automation —
    // the previous version fetched it twice (one extra SQL roundtrip per
    // move) and had a small TOCTOU window where auto_trigger could
    // toggle between the two reads.
    let dest_column: Option<Column> = fields
        .column_id
        .as_deref()
        .map(|dest_id| ColumnStore::get_by_id(&state.db, dest_id))
        .transpose()?;

    // Pre-check: if the move targets an auto-trigger column, validate
    // provider + specialist BEFORE the move commits. A 4xx here leaves
    // the task in its current column — recoverable, no orphan state.
    if let Some(ref col) = dest_column {
        if col.auto_trigger {
            try_automate_lane_precheck(&state, col)?;
        }
    }
    let mut updated = apply_task_update(&state.db, &id, &fields)?;

    // Lane automation (after the move, when targeting an auto-trigger
    // column). Errors propagate as 4xx/5xx.
    if let Some(ref col) = dest_column {
        if col.auto_trigger {
            try_automate_lane(&state, &updated, col).await?;
            // Re-fetch: try_automate_lane sets `session_id` on the task.
            let workspace_id = lookup_workspace_for_task(&state.db, &id)?;
            updated = TaskStore::get_by_id(&state.db, &id, &workspace_id)?;
        }
    }

    // Broadcast the board-scoped lifecycle event. Use `task_moved` for
    // column changes (carries from/to), `task_updated` otherwise.
    let board_id = updated.board_id.clone();
    if column_changed {
        let from_column_id = old_column_id.unwrap_or_default();
        let to_column_id = updated.column_id.clone();
        let task_json = serde_json::to_value(&updated).map_err(|e| {
            AppError::Internal(anyhow::anyhow!("failed to serialize task for SSE: {e}"))
        })?;
        state.sse_manager.broadcast(
            &format!("board:{}", board_id),
            SseWireEvent::TaskMoved {
                task: task_json,
                from_column_id,
                to_column_id,
            },
        );
    } else {
        let task_json = serde_json::to_value(&updated).map_err(|e| {
            AppError::Internal(anyhow::anyhow!("failed to serialize task for SSE: {e}"))
        })?;
        state.sse_manager.broadcast(
            &format!("board:{}", board_id),
            SseWireEvent::TaskUpdated { task: task_json },
        );
    }
    Ok(Json(DataResponse { data: updated }))
}

/// DELETE /api/tasks/{tid}
pub async fn delete_task(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<()>>, AppError> {
    // Find the task's workspace + board + column for scoping and
    // for the SSE broadcast payload. 2-query overhead.
    let workspace_id = lookup_workspace_for_task(&state.db, &id)?;
    let column_id = lookup_task_column_id(&state.db, &id)?;
    let board_id: String = state
        .db
        .conn()
        .query_row("SELECT board_id FROM tasks WHERE id = ?1", [&id], |r| {
            r.get(0)
        })
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                resource: "task".into(),
                id: id.clone(),
            },
            other => other.into(),
        })?;
    TaskStore::delete(&state.db, &id, &workspace_id)?;
    state.sse_manager.broadcast(
        &format!("board:{}", board_id),
        SseWireEvent::TaskDeleted {
            task_id: id,
            column_id,
        },
    );
    Ok(Json(DataResponse { data: () }))
}

// ---------------------------------------------------------------------------
// SSE handler
// ---------------------------------------------------------------------------

/// Cadence of the explicit JSON heartbeat emitted on the board stream.
///
/// 15s matches the spec (`feature_list.json:256`). The comment-style
/// keep-alive from axum's `KeepAlive::default()` is invisible to JS;
/// the spec wants an observable `{"type":"heartbeat"}` event.
const BOARD_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// Build the `{"type":"heartbeat"}` SSE event for the board stream.
///
/// Factored out so the shape can be tested without sleeping 15s.
fn build_heartbeat_event() -> Event {
    let data = serde_json::to_string(&SseWireEvent::Heartbeat {})
        .unwrap_or_else(|_| "{\"type\":\"heartbeat\"}".to_string());
    Event::default().event("heartbeat").data(data)
}

/// GET /api/boards/{bid}/stream
///
/// SSE stream of board-scoped events (`task_created`, `task_moved`,
/// `task_updated`, `task_deleted`, `column_added`, `session_started`).
/// Emits a `connected` event on mount, replays buffered events on
/// reconnect with `Last-Event-ID`, then transitions to live. A JSON
/// `{"type":"heartbeat"}` event is emitted every 15s.
pub async fn board_stream(
    axum::Extension(state): axum::Extension<AppState>,
    Path(board_id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let board_exists = BoardStore::get_by_id(&state.db, &board_id).is_ok();
    let last_event_id: Option<u64> = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let sse_manager = state.sse_manager.clone();
    let bid = board_id.clone();

    // State machine for the board stream. Mirrors `session_stream`'s
    // shape but uses `board:{bid}` as the entity id and races the
    // live receiver against a 15s heartbeat.
    enum BoardSseState {
        Initial,
        Buffered(
            Vec<crate::sse::BufferedEvent>,
            usize,
            tokio::sync::broadcast::Receiver<SseWireEvent>,
            u64,
        ),
        /// `next_heartbeat` is the absolute `Instant` of the next heartbeat
        /// emission. The deadline is preserved across loop iterations so a
        /// flood of live events cannot reset the cadence.
        Live(
            tokio::sync::broadcast::Receiver<SseWireEvent>,
            u64,
            tokio::time::Instant,
        ),
        Done,
    }

    let sse_stream = stream::unfold(
        (
            sse_manager,
            bid,
            last_event_id,
            BoardSseState::Initial,
            board_exists,
        ),
        |(mgr, bid, last_id, state, board_exists)| async move {
            match state {
                BoardSseState::Initial => {
                    if !board_exists {
                        let error_data = serde_json::to_string(&SseWireEvent::Error {
                            message: "board not found".into(),
                        })
                        .unwrap_or_default();
                        let err = Event::default().event("error").data(error_data);
                        return Some((Ok(err), (mgr, bid, last_id, BoardSseState::Done, false)));
                    }

                    // Connected event (no ID — protocol marker, not replayed).
                    // The `session_id` field is empty for board streams; the
                    // board id is in the URL.
                    let connected_data = serde_json::to_string(&SseWireEvent::Connected {
                        session_id: String::new(),
                    })
                    .unwrap_or_default();
                    let connected = Event::default().event("connected").data(connected_data);

                    let entity = format!("board:{}", bid);
                    let rx = mgr.subscribe(&entity);

                    // Skip replay on a fresh mount (no Last-Event-ID) so
                    // the frontend doesn't re-apply stale events from a
                    // prior session. Reconnects WITH Last-Event-ID
                    // receive buffered events.
                    let buffered = match last_id {
                        Some(after_id) => mgr.get_after(&entity, after_id),
                        None => Vec::new(),
                    };
                    let max_id = buffered.last().map(|e| e.id).unwrap_or(0);
                    if !buffered.is_empty() {
                        Some((
                            Ok(connected),
                            (
                                mgr,
                                bid,
                                last_id,
                                BoardSseState::Buffered(buffered, 0, rx, max_id),
                                true,
                            ),
                        ))
                    } else {
                        let first_heartbeat =
                            tokio::time::Instant::now() + BOARD_HEARTBEAT_INTERVAL;
                        Some((
                            Ok(connected),
                            (
                                mgr,
                                bid,
                                last_id,
                                BoardSseState::Live(rx, max_id, first_heartbeat),
                                true,
                            ),
                        ))
                    }
                }
                BoardSseState::Buffered(buffered, idx, rx, max_id) => {
                    if idx < buffered.len() {
                        let entry = &buffered[idx];
                        let event = Event::default()
                            .id(entry.id.to_string())
                            .event(&entry.event_type)
                            .data(&entry.data);
                        Some((
                            Ok(event),
                            (
                                mgr,
                                bid,
                                last_id,
                                BoardSseState::Buffered(buffered, idx + 1, rx, max_id),
                                true,
                            ),
                        ))
                    } else {
                        // Buffered done — transition to live.
                        let complete = Event::default().event("buffered_complete").data("{}");
                        let first_heartbeat =
                            tokio::time::Instant::now() + BOARD_HEARTBEAT_INTERVAL;
                        Some((
                            Ok(complete),
                            (
                                mgr,
                                bid,
                                last_id,
                                BoardSseState::Live(rx, max_id, first_heartbeat),
                                true,
                            ),
                        ))
                    }
                }
                BoardSseState::Live(mut rx, max_buffered_id, next_heartbeat) => loop {
                    // Track the next heartbeat deadline across iterations.
                    // The sleep is created from the absolute deadline so a
                    // flood of live events does not reset the cadence — the
                    // `select!` only re-arms the future, never the deadline.
                    // The `biased` ordering polls the heartbeat first so a
                    // long burst of events can't starve it.
                    if tokio::time::Instant::now() >= next_heartbeat {
                        let next = next_heartbeat + BOARD_HEARTBEAT_INTERVAL;
                        return Some((
                            Ok(build_heartbeat_event()),
                            (
                                mgr,
                                bid,
                                last_id,
                                BoardSseState::Live(rx, max_buffered_id, next),
                                true,
                            ),
                        ));
                    }
                    let heartbeat = tokio::time::sleep_until(next_heartbeat);
                    tokio::select! {
                        biased;
                        _ = heartbeat => {
                            let next = next_heartbeat + BOARD_HEARTBEAT_INTERVAL;
                            return Some((
                                Ok(build_heartbeat_event()),
                                (mgr, bid, last_id, BoardSseState::Live(rx, max_buffered_id, next), true),
                            ));
                        }
                        recv = rx.recv() => {
                            match recv {
                                Ok(wire_event) => {
                                    let current_id = mgr.get_current_id(&format!("board:{}", bid));
                                    let event_id = current_id.saturating_sub(1);
                                    if event_id <= max_buffered_id {
                                        continue;
                                    }
                                    let event_type = wire_event.event_type().to_string();
                                    let data = crate::sse::sse_data(&wire_event);
                                    let event = Event::default()
                                        .id(event_id.to_string())
                                        .event(&event_type)
                                        .data(&data);
                                    return Some((
                                        Ok(event),
                                        (
                                            mgr,
                                            bid,
                                            last_id,
                                            BoardSseState::Live(rx, max_buffered_id, next_heartbeat),
                                            true,
                                        ),
                                    ));
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                    let gap_data = serde_json::to_string(&SseWireEvent::Gap { missed: n })
                                        .unwrap_or_default();
                                    let gap = Event::default().event("gap").data(gap_data);
                                    return Some((
                                        Ok(gap),
                                        (
                                            mgr,
                                            bid,
                                            last_id,
                                            BoardSseState::Live(rx, max_buffered_id, next_heartbeat),
                                            true,
                                        ),
                                    ));
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    return None;
                                }
                            }
                        }
                    }
                },
                BoardSseState::Done => None,
            }
        },
    );

    Sse::new(sse_stream)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn validate_board_name(name: &str) -> Result<(), AppError> {
    if name.is_empty() {
        return Err(AppError::validation("board name must not be empty"));
    }
    if name.chars().count() > MAX_BOARD_NAME_LEN {
        return Err(AppError::validation(format!(
            "board name must be at most {} characters",
            MAX_BOARD_NAME_LEN
        )));
    }
    Ok(())
}

/// Apply a task update. If `column_id` is being changed, route through
/// `move_to_column` (which triggers rebalance). Otherwise call
/// `TaskStore::update`. Workspace is derived from the task's board.
fn apply_task_update(
    db: &std::sync::Arc<crate::db::Db>,
    task_id: &str,
    fields: &UpdateTask,
) -> Result<Task, AppError> {
    let workspace_id = lookup_workspace_for_task(db, task_id)?;

    if let Some(ref new_col) = fields.column_id {
        // Move the task (also triggers position rebalance).
        TaskStore::move_to_column(db, task_id, &workspace_id, new_col, fields.position)?;
        // Apply the remaining fields, but skip position (already set
        // by move_to_column) and column_id (already handled).
        let rest = fields_without_column(fields);
        return TaskStore::update(db, task_id, &workspace_id, &rest);
    }

    TaskStore::update(db, task_id, &workspace_id, fields)
}

/// Strip `column_id` (and position, which is handled by the move) from
/// an `UpdateTask`, returning a new value to pass to the regular `update`.
fn fields_without_column(fields: &UpdateTask) -> UpdateTask {
    let mut f = fields.clone();
    f.column_id = None;
    f.position = None;
    f
}

/// Look up the workspace id for a task via its board.
fn lookup_workspace_for_task(
    db: &std::sync::Arc<crate::db::Db>,
    task_id: &str,
) -> Result<String, AppError> {
    db.conn()
        .query_row(
            "SELECT b.workspace_id FROM tasks t
             JOIN boards b ON b.id = t.board_id
             WHERE t.id = ?1",
            [task_id],
            |r| r.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                resource: "task".into(),
                id: task_id.into(),
            },
            other => other.into(),
        })
}

/// Look up the current column id for a task. Used to capture
/// `from_column_id` for the `task_moved` SSE broadcast.
fn lookup_task_column_id(
    db: &std::sync::Arc<crate::db::Db>,
    task_id: &str,
) -> Result<String, AppError> {
    db.conn()
        .query_row(
            "SELECT column_id FROM tasks WHERE id = ?1",
            [task_id],
            |r| r.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                resource: "task".into(),
                id: task_id.into(),
            },
            other => other.into(),
        })
}

/// Pre-check the destination column for an auto-trigger move.
///
/// Runs BEFORE the move commits so a setup failure (no provider,
/// specialist not loaded) leaves the task in its current column.
/// Returns the `AppError` that would otherwise surface from
/// `try_automate_lane` so the user sees a 400 before any state changes.
fn try_automate_lane_precheck(state: &AppState, column: &Column) -> Result<(), AppError> {
    // The precheck mirrors the early-bail logic in `try_automate_lane`:
    // if either invariant fails we surface the same 400 the user would
    // have gotten post-move, but with the task still in the old column.
    let specialist_id = match column.specialist_id.as_deref() {
        Some(id) if !id.is_empty() => id,
        // Defensive: validate_auto_trigger enforces this at column create/update.
        _ => return Ok(()),
    };
    if state.specialists.get_by_name(specialist_id).is_none() {
        return Err(AppError::validation(format!(
            "specialist '{specialist_id}' is not loaded; check resources/specialists/ \
             for a markdown file with `name: {specialist_id}` in its frontmatter"
        )));
    }
    // Provider check is cheap; do it here too so the user sees the
    // clearer "no provider" error before the move.
    if ProviderStore::list(&state.db)?.is_empty() {
        return Err(AppError::validation(
            "no provider configured in workspace; add one via POST /api/providers \
             before moving tasks to auto-trigger columns",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::make_test_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use serde_json::Value;
    use tower::ServiceExt;

    fn test_app(state: AppState) -> Router {
        Router::new()
            .route(
                "/api/workspaces/{wid}/boards",
                axum::routing::get(list_boards).post(create_board),
            )
            .route(
                "/api/workspaces/{wid}/boards/{id}",
                axum::routing::get(get_board)
                    .patch(update_board)
                    .delete(delete_board),
            )
            .route(
                "/api/workspaces/{wid}/boards/{bid}/columns",
                axum::routing::post(create_column),
            )
            .route("/api/columns/{id}", axum::routing::patch(update_column))
            .route(
                "/api/workspaces/{wid}/boards/{bid}/cards",
                axum::routing::post(create_card),
            )
            .route(
                "/api/tasks/{id}",
                axum::routing::patch(update_task).delete(delete_task),
            )
            .route("/api/boards/{bid}/stream", axum::routing::get(board_stream))
            .route(
                "/api/workspaces/{wid}/tasks",
                axum::routing::get(list_unbound_tasks),
            )
            .layer(axum::Extension(state))
    }

    fn extract_json(body: &[u8]) -> Value {
        serde_json::from_slice(body).unwrap()
    }

    #[tokio::test]
    async fn test_kanban_crud() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let app = test_app(state);

        // CREATE board with template columns
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/boards", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"My Project","columns":[{"name":"To Do","position":1024}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::CREATED);
        let board_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // GET board (composite)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/boards/{}", ws_id, board_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail = extract_json(&body)["data"].clone();
        assert_eq!(detail["board"]["id"], board_id);
        let col_id = detail["columns"][0]["id"].as_str().unwrap().to_string();

        // CREATE card
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{}/boards/{}/cards",
                        ws_id, board_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"column_id":"{}","title":"Card 1"}}"#,
                        col_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let task_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // PATCH card (rename)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/tasks/{}", task_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"Renamed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // DELETE card
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/tasks/{}", task_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // DELETE board
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/workspaces/{}/boards/{}", ws_id, board_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_board_with_template_and_get() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let app = test_app(state);

        // CREATE board with template columns
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/boards", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Project","columns":[{"name":"To Do"},{"name":"Done"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let board: Value = extract_json(&body)["data"].clone();
        let board_id = board["id"].as_str().unwrap().to_string();
        assert_eq!(board["name"], "Project");

        // GET board (composite)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/boards/{}", ws_id, board_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail = extract_json(&body)["data"].clone();
        assert_eq!(detail["board"]["id"], board_id);
        assert_eq!(detail["columns"].as_array().unwrap().len(), 2);
        assert_eq!(detail["tasks"].as_array().unwrap().len(), 0);

        // CREATE card
        let col_id = detail["columns"][0]["id"].as_str().unwrap().to_string();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{}/boards/{}/cards",
                        ws_id, board_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"column_id":"{}","title":"My first card"}}"#,
                        col_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let task = extract_json(&body)["data"].clone();
        let task_id = task["id"].as_str().unwrap().to_string();
        assert_eq!(task["title"], "My first card");

        // PATCH card (rename)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/tasks/{}", task_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"Renamed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(extract_json(&body)["data"]["title"], "Renamed");

        // DELETE card
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/tasks/{}", task_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // DELETE board (cascades to columns)
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/workspaces/{}/boards/{}", ws_id, board_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_kanban_column_ordering() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let app = test_app(state);

        // Create board with 3 columns at positions 1024, 2048, 3072
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/boards", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Col Order","columns":[
                            {"name":"Third","position":3072},
                            {"name":"First","position":1024},
                            {"name":"Second","position":2048}
                        ]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let board_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // GET the board; columns should come back in position order
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/boards/{}", ws_id, board_id))
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
        let columns = json["data"]["columns"].as_array().unwrap();
        assert_eq!(columns.len(), 3);
        assert_eq!(columns[0]["name"], "First");
        assert_eq!(columns[1]["name"], "Second");
        assert_eq!(columns[2]["name"], "Third");
    }

    #[tokio::test]
    async fn test_kanban_task_position() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let app = test_app(state);

        // Create board + column
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/boards", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Pos","columns":[{"name":"C1"}]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let board_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Create column directly (since template only had 1)
        let col_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{}/boards/{}/columns",
                        ws_id, board_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"C2","position":2048}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(col_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let c2_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Get c1 id
        let detail_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/boards/{}", ws_id, board_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(detail_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let c1_id = extract_json(&body)["data"]["columns"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Create 3 tasks in C1
        let mut task_ids = Vec::new();
        for i in 1..=3 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!(
                            "/api/workspaces/{}/boards/{}/cards",
                            ws_id, board_id
                        ))
                        .header("content-type", "application/json")
                        .body(Body::from(format!(
                            r#"{{"column_id":"{}","title":"T{}"}}"#,
                            c1_id, i
                        )))
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            task_ids.push(
                extract_json(&body)["data"]["id"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }

        // Move task 1 from C1 to C2 (using PATCH /api/tasks/:tid with column_id)
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/tasks/{}", task_ids[0]))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"column_id":"{}"}}"#, c2_id)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Re-fetch the board; c2 should have 1 task, c1 should have 2
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/boards/{}", ws_id, board_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail = extract_json(&body);
        let tasks = detail["data"]["tasks"].as_array().unwrap();
        let c1_tasks: Vec<&Value> = tasks.iter().filter(|t| t["column_id"] == c1_id).collect();
        let c2_tasks: Vec<&Value> = tasks.iter().filter(|t| t["column_id"] == c2_id).collect();
        assert_eq!(c1_tasks.len(), 2, "C1 should have 2 tasks");
        assert_eq!(c2_tasks.len(), 1, "C2 should have 1 task");
    }

    #[tokio::test]
    async fn test_auto_trigger_guard_returns_400() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let app = test_app(state);

        // Create board
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/boards", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"X"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let board_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Create column with auto_trigger=true and no specialist_id
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{}/boards/{}/columns",
                        ws_id, board_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Bad","auto_trigger":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_board_validates_name() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let app = test_app(state);

        // Empty name
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/boards", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_get_board_not_found() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let app = test_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/boards/nonexistent", ws_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_update_task_moves_between_columns() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let app = test_app(state);

        // Create board with 2 columns
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/boards", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"M","columns":[{"name":"A"},{"name":"B"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let board_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // GET board to fetch the columns
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/boards/{}", ws_id, board_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail = extract_json(&body)["data"].clone();
        let c_a = detail["columns"][0]["id"].as_str().unwrap().to_string();
        let c_b = detail["columns"][1]["id"].as_str().unwrap().to_string();

        // Create card in A
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{}/boards/{}/cards",
                        ws_id, board_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"column_id":"{}","title":"Card"}}"#,
                        c_a
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let task_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Move to B
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/tasks/{}", task_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"column_id":"{}"}}"#, c_b)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let task = extract_json(&body)["data"].clone();
        assert_eq!(task["column_id"], c_b);
    }

    #[tokio::test]
    async fn test_delete_task_returns_404_for_missing() {
        let state = make_test_state();
        let app = test_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/tasks/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_cross_workspace_board_access_returns_404() {
        // Board belongs to ws A; we request it from ws B (wrong workspace
        // id in the URL) and must get 404, not 200 with the data.
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let app = test_app(state);

        // Create a board in the real workspace
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/boards", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"X"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let board_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // GET board with the WRONG workspace id
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/wrong-ws/boards/{}", board_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // PATCH board with the wrong workspace id
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/workspaces/wrong-ws/boards/{}", board_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Hacked"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // DELETE board with the wrong workspace id
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/workspaces/wrong-ws/boards/{}", board_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // Sanity check: the real workspace can still see the board
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/boards/{}", ws_id, board_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_task_with_column_from_other_board_returns_400() {
        // A task in board A cannot be created in a column from board B,
        // even within the same workspace.
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let app = test_app(state);

        // Create board A with one column
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/boards", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"A","columns":[{"name":"A1"}]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let a_board_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Create board B with one column
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/workspaces/{}/boards", ws_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"B","columns":[{"name":"B1"}]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let b_board_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // GET board B to fetch its column
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/boards/{}", ws_id, b_board_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let b_col_id = extract_json(&body)["data"]["columns"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Try to create a card in board A but with a column from board B
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{}/boards/{}/cards",
                        ws_id, a_board_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"column_id":"{}","title":"Bad"}}"#,
                        b_col_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // --- feat-025: lane automation + board SSE tests ---

    /// End-to-end lane automation. Subscribes to the board's SSE channel,
    /// moves a card into an auto-trigger column, and asserts that:
    /// (a) the move returns 200, (b) the task's `session_id` is set,
    /// (c) a session row exists with the right specialist_id, (d) a
    /// `task_moved` event was broadcast, (e) a `session_started` event
    /// was broadcast. This is the verification target for feat-025.
    #[tokio::test]
    async fn test_lane_automation() {
        use crate::store::kanban_test_helpers::seed_provider_and_specialist;

        let mut state = make_test_state();
        let (_provider_id, specialist_name) = seed_provider_and_specialist(&mut state, "dev");

        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();

        // Subscribe to the board channel BEFORE performing the move,
        // otherwise we'd miss the broadcast.
        let board_id = "test-board-id".to_string();
        let mut rx = state.sse_manager.subscribe(&format!("board:{}", board_id));

        // Insert a board with two columns; col-2 has auto_trigger=true.
        let now = chrono::Utc::now().to_rfc3339();
        let col1_id = "col-1".to_string();
        let col2_id = "col-2".to_string();
        state
            .db
            .conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at) VALUES (?1, ?2, 'b', ?3)",
                rusqlite::params![board_id, ws_id, now],
            )
            .unwrap();
        state.db.conn().execute(
            "INSERT INTO columns (id, board_id, name, position, specialist_id, auto_trigger, created_at)
             VALUES (?1, ?2, 'plain', 0, NULL, 0, ?3)",
            rusqlite::params![col1_id, board_id, now],
        ).unwrap();
        state.db.conn().execute(
            "INSERT INTO columns (id, board_id, name, position, specialist_id, auto_trigger, created_at)
             VALUES (?1, ?2, 'auto', 1024, ?3, 1, ?4)",
            rusqlite::params![col2_id, board_id, specialist_name, now],
        ).unwrap();

        // Create a card in col-1 via the API.
        let app = test_app(state.clone());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{}/boards/{}/cards",
                        ws_id, board_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"column_id":"{}","title":"Implement auth","description":"use JWT"}}"#,
                        col1_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::CREATED);
        let task_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Move the card to col-2 (auto-trigger).
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/tasks/{}", task_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"column_id":"{}"}}"#, col2_id)))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        let task_json = extract_json(&body)["data"].clone();
        let session_id = task_json["session_id"].as_str();
        assert!(
            session_id.is_some(),
            "task.session_id should be set after lane automation"
        );

        // Assert a session row exists with the right specialist.
        let session =
            crate::store::sessions::SessionStore::get_by_id(&state.db, session_id.unwrap())
                .unwrap();
        assert_eq!(
            session.specialist_id.as_deref(),
            Some(specialist_name.as_str())
        );

        // Assert the user message was persisted with the expected prompt.
        let msgs = crate::store::sessions::MessageStore::list_by_session(
            &state.db,
            session_id.unwrap(),
            None,
            10,
        )
        .unwrap();
        assert_eq!(msgs.data.len(), 1);
        assert_eq!(msgs.data[0].role, "user");
        assert_eq!(
            msgs.data[0].content,
            "Process task: Implement auth\nuse JWT"
        );

        // Drain the SSE receiver and assert task_moved + session_started arrived.
        // Skip TaskCreated (the card creation also broadcast one) and other
        // non-target events with `continue` so we don't break on the first
        // non-matching variant.
        let mut got_moved = false;
        let mut got_started = false;
        for _ in 0..10 {
            if got_moved && got_started {
                break;
            }
            match tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await {
                Ok(Ok(SseWireEvent::TaskMoved {
                    task,
                    from_column_id,
                    to_column_id,
                })) => {
                    assert_eq!(task["id"].as_str().unwrap(), task_id);
                    assert_eq!(from_column_id, col1_id);
                    assert_eq!(to_column_id, col2_id);
                    got_moved = true;
                }
                Ok(Ok(SseWireEvent::SessionStarted {
                    session_id: sid,
                    task_id: tid,
                    specialist_id,
                    board_id: bid,
                })) => {
                    assert_eq!(sid, session_id.unwrap());
                    assert_eq!(tid, task_id);
                    assert_eq!(specialist_id, specialist_name);
                    assert_eq!(bid, board_id);
                    got_started = true;
                }
                Ok(Ok(_)) => continue, // skip TaskCreated, etc.
                _ => break,
            }
        }
        assert!(got_moved, "expected task_moved SSE event");
        assert!(got_started, "expected session_started SSE event");
    }

    /// Defense-in-depth: no provider seeded → 400 before move.
    #[tokio::test]
    async fn test_lane_automation_no_provider_returns_400() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let board_id = "b1".to_string();
        let col1_id = "c1".to_string();
        let col2_id = "c2".to_string();
        let now = chrono::Utc::now().to_rfc3339();
        state
            .db
            .conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at) VALUES (?1, ?2, 'b', ?3)",
                rusqlite::params![board_id, ws_id, now],
            )
            .unwrap();
        state.db.conn().execute(
            "INSERT INTO columns (id, board_id, name, position, specialist_id, auto_trigger, created_at)
             VALUES (?1, ?2, 'plain', 0, NULL, 0, ?3)",
            rusqlite::params![col1_id, board_id, now],
        ).unwrap();
        state.db.conn().execute(
            "INSERT INTO columns (id, board_id, name, position, specialist_id, auto_trigger, created_at)
             VALUES (?1, ?2, 'auto', 1024, 'dev', 1, ?3)",
            rusqlite::params![col2_id, board_id, now],
        ).unwrap();

        let app = test_app(state.clone());
        // Create a card in col-1
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{}/boards/{}/cards",
                        ws_id, board_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"column_id":"{}","title":"T"}}"#,
                        col1_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let task_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Move to col-2 (no provider, no specialist) → 400
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/tasks/{}", task_id))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"column_id":"{}"}}"#, col2_id)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// All 5 kanban SSE events broadcast on their respective CRUD calls.
    #[tokio::test]
    async fn test_kanban_sse_events() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let board_id = "b-sse".to_string();
        let mut rx = state.sse_manager.subscribe(&format!("board:{}", board_id));
        let now = chrono::Utc::now().to_rfc3339();
        let col1_id = "c-sse-1".to_string();
        state
            .db
            .conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at) VALUES (?1, ?2, 'b', ?3)",
                rusqlite::params![board_id, ws_id, now],
            )
            .unwrap();
        state.db.conn().execute(
            "INSERT INTO columns (id, board_id, name, position, specialist_id, auto_trigger, created_at)
             VALUES (?1, ?2, 'plain', 0, NULL, 0, ?3)",
            rusqlite::params![col1_id, board_id, now],
        ).unwrap();

        let app = test_app(state.clone());
        // CREATE card → task_created
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{}/boards/{}/cards",
                        ws_id, board_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"column_id":"{}","title":"T"}}"#,
                        col1_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let task_id = extract_json(&body)["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // PATCH title only → task_updated
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/tasks/{}", task_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"Renamed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // DELETE task → task_deleted
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/tasks/{}", task_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // CREATE column → column_added
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{}/boards/{}/columns",
                        ws_id, board_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"another"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Drain and assert each variant was received.
        let mut got_created = false;
        let mut got_updated = false;
        let mut got_deleted = false;
        let mut got_column_added = false;
        for _ in 0..10 {
            match tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await {
                Ok(Ok(SseWireEvent::TaskCreated { .. })) => got_created = true,
                Ok(Ok(SseWireEvent::TaskUpdated { .. })) => got_updated = true,
                Ok(Ok(SseWireEvent::TaskDeleted { .. })) => got_deleted = true,
                Ok(Ok(SseWireEvent::ColumnAdded { .. })) => got_column_added = true,
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }
        assert!(got_created, "expected task_created event");
        assert!(got_updated, "expected task_updated event");
        assert!(got_deleted, "expected task_deleted event");
        assert!(got_column_added, "expected column_added event");
    }

    /// The heartbeat event builder produces the spec-mandated shape.
    #[test]
    fn test_heartbeat_event_shape() {
        // We can't easily inspect an axum Event's internals; assert the
        // call doesn't panic and the JSON wire shape is right.
        let _event = build_heartbeat_event();
        let data = serde_json::to_string(&SseWireEvent::Heartbeat {}).unwrap();
        assert_eq!(data, "{\"type\":\"heartbeat\"}");
    }

    /// feat-053: the `?unbound=true` filter is mandatory. A request
    /// with no query (or `?unbound=false`) is rejected with 400 + the
    /// `missing_unbound_filter` code, because the endpoint exists for
    /// one purpose and the strict grammar keeps the URL deterministic.
    /// The happy path returns the workspace's active+unbound tasks
    /// (empty Vec in the default test workspace).
    #[tokio::test]
    async fn test_unbound_tasks_endpoint_rejects_missing_or_false_param() {
        let state = make_test_state();
        let ws_id: String = state
            .db
            .conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let app = test_app(state);

        // No query → 400.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/tasks", ws_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert_eq!(
            json.pointer("/error/code").and_then(|v| v.as_str()),
            Some("missing_unbound_filter")
        );

        // Explicit `?unbound=false` → 400 (same code).
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/tasks?unbound=false", ws_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert_eq!(
            json.pointer("/error/code").and_then(|v| v.as_str()),
            Some("missing_unbound_filter")
        );

        // Happy path: `?unbound=true` → 200 with empty `data` array.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workspaces/{}/tasks?unbound=true", ws_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json = extract_json(&body);
        assert!(json.get("data").map(|d| d.is_array()).unwrap_or(false));
        assert_eq!(
            json.get("data").and_then(|d| d.as_array()).map(|a| a.len()),
            Some(0)
        );
    }
}
