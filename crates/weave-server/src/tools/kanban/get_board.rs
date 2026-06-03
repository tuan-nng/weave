//! `get_board` — return the composite state of a single board.
//!
//! Mirrors the HTTP `GET /api/workspaces/{wid}/boards/{id}` response
//! shape: `{board, columns, tasks}`. Cross-workspace access is
//! rejected as `Not found: board <id>` (matches the HTTP handler's
//! defense at `api/kanban.rs:179-186`).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::boards::BoardStore;
use crate::tools::fs::{error, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct GetBoardTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for GetBoardTool {
    fn name(&self) -> &str {
        "get_board"
    }

    fn description(&self) -> &str {
        "Get the full state of a board: its columns and all tasks. \
         Returns board metadata, ordered columns, and a flat task list \
         (the client groups by column_id)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "board_id": {
                    "type": "string",
                    "description": "The board ID to retrieve. Must belong to the current workspace."
                }
            },
            "required": ["board_id"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let board_id = match require_string(&input, "board_id") {
            Ok(s) => s,
            Err(e) => return e,
        };

        // Cross-workspace defense via the canonical helper: a board
        // owned by a different workspace returns NotFound (not 403)
        // so the agent cannot enumerate boards across workspaces.
        if BoardStore::get_in_workspace(&self.db, &board_id, &ctx.workspace_id).is_err() {
            return error(format!("Not found: board {board_id}"));
        }

        match BoardStore::get_with_children(&self.db, &board_id) {
            Ok(detail) => success(json!({
                "board": detail.board,
                "columns": detail.columns,
                "tasks": detail.tasks,
            })),
            Err(e) => error(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::{make_test_db, seed_workspace_with_board};
    use crate::tools::test_support::{make_context, make_context_for_workspace};
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_get_board_success() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = GetBoardTool { db };
        let result = tool.execute(json!({"board_id": bid}), &ctx).await;

        assert!(result.success, "got: {:?}", result.error);
        assert_eq!(result.data["board"]["id"], bid);
        assert_eq!(result.data["board"]["workspace_id"], ws);
        assert!(result.data["columns"].is_array());
        assert!(result.data["tasks"].is_array());
    }

    #[tokio::test]
    async fn test_get_board_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        seed_workspace_with_board(&db);

        let tool = GetBoardTool { db };
        let result = tool.execute(json!({"board_id": "nonexistent"}), &ctx).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_get_board_missing_id() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();

        let tool = GetBoardTool { db };
        let result = tool.execute(json!({}), &ctx).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_get_board_cross_workspace_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path()); // workspace_id = "test-workspace"
        let db = make_test_db();
        // Seed a board in a different workspace.
        let now = chrono::Utc::now().to_rfc3339();
        let other_ws = "other-workspace";
        let bid = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'other', 'active', ?2, ?2)",
                rusqlite::params![other_ws, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at)
                 VALUES (?1, ?2, 'foreign-board', ?3)",
                rusqlite::params![bid, other_ws, now],
            )
            .unwrap();

        let tool = GetBoardTool { db };
        let result = tool.execute(json!({"board_id": bid}), &ctx).await;

        assert!(!result.success, "cross-workspace access must be rejected");
        assert!(
            result.error.unwrap().contains("Not found"),
            "should return NotFound, not leak existence"
        );
    }
}
