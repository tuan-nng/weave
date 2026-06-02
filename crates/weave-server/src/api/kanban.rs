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
//!
//! All board-scoped routes take `wid` in the URL and the handler
//! verifies the board's `workspace_id` matches before proceeding.
//! This prevents cross-workspace data access by guessed UUID.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::api::responses::DataResponse;
use crate::error::AppError;
use crate::store::boards::{BoardStore, NewColumnSpec};
use crate::store::columns::ColumnStore;
use crate::store::tasks::{Task, TaskStore, UpdateTask};
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
}

#[derive(Deserialize)]
pub struct UpdateColumnRequest {
    pub name: Option<String>,
    pub position: Option<i64>,
    /// `Some(None)` clears the specialist binding. `None` leaves it unchanged.
    pub specialist_id: Option<Option<String>>,
    pub auto_trigger: Option<bool>,
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
                return Err(AppError::Validation("column name must not be empty".into()));
            }
            if trimmed.chars().count() > MAX_COLUMN_NAME_LEN {
                return Err(AppError::Validation(format!(
                    "column name must be at most {} characters",
                    MAX_COLUMN_NAME_LEN
                )));
            }
            Ok(NewColumnSpec {
                name: trimmed,
                position: c.position,
                specialist_id: c.specialist_id.as_deref(),
                auto_trigger: c.auto_trigger.unwrap_or(false),
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
        return Err(AppError::Validation("column name must not be empty".into()));
    }
    if name.chars().count() > MAX_COLUMN_NAME_LEN {
        return Err(AppError::Validation(format!(
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
    )?;
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
            return Err(AppError::Validation("column name must not be empty".into()));
        }
        if n.chars().count() > MAX_COLUMN_NAME_LEN {
            return Err(AppError::Validation(format!(
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
        return Err(AppError::Validation("task title must not be empty".into()));
    }
    if title.chars().count() > MAX_TASK_TITLE_LEN {
        return Err(AppError::Validation(format!(
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
    Ok((StatusCode::CREATED, Json(DataResponse { data: task })))
}

/// PATCH /api/tasks/{tid}
///
/// Supports any subset of the editable fields. Use `column_id` to
/// move a task between columns; the move triggers an automatic
/// position rebalance in the target column.
pub async fn update_task(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskRequest>,
) -> Result<Json<DataResponse<Task>>, AppError> {
    // Workspace scoping: the workspace comes from the board, which
    // comes from the task. Fetch the task first to find its workspace.
    // (No workspace_id on the URL; the API must derive it from the task.)
    let fields = UpdateTask::from(body);
    let updated = apply_task_update(&state.db, &id, &fields)?;
    Ok(Json(DataResponse { data: updated }))
}

/// DELETE /api/tasks/{tid}
pub async fn delete_task(
    axum::Extension(state): axum::Extension<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DataResponse<()>>, AppError> {
    // Find the task's workspace for scoping. This is a 2-query
    // overhead (find workspace, then delete scoped). Acceptable.
    let workspace_id = lookup_workspace_for_task(&state.db, &id)?;
    TaskStore::delete(&state.db, &id, &workspace_id)?;
    Ok(Json(DataResponse { data: () }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn validate_board_name(name: &str) -> Result<(), AppError> {
    if name.is_empty() {
        return Err(AppError::Validation("board name must not be empty".into()));
    }
    if name.chars().count() > MAX_BOARD_NAME_LEN {
        return Err(AppError::Validation(format!(
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
}
