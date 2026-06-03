//! `create_card` — create a new task on a column.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::boards::BoardStore;
use crate::store::tasks::{TaskStore, VALID_TASK_STATUSES};
use crate::tools::fs::{
    check_optional_status, error, optional_string, require_string, success, validate_task_title,
};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct CreateCardTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for CreateCardTool {
    fn name(&self) -> &str {
        "create_card"
    }

    fn description(&self) -> &str {
        "Create a new card (task) on a column. \
         Returns the created card. \
         The card's board is inferred from its column; \
         the board is verified to belong to the current workspace."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "board_id": {
                    "type": "string",
                    "description": "The board ID. Used to verify the column belongs to this board and the board is in the current workspace."
                },
                "column_id": {
                    "type": "string",
                    "description": "The column to place the new card in. Must belong to board_id."
                },
                "title": {
                    "type": "string",
                    "description": "Card title. 1-500 characters after trim."
                },
                "description": {
                    "type": "string",
                    "description": "Optional longer description of the card."
                },
                "status": {
                    "type": "string",
                    "description": format!(
                        "Optional. Defaults to 'active'. Valid: {}",
                        VALID_TASK_STATUSES.join(", ")
                    )
                }
            },
            "required": ["board_id", "column_id", "title"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let board_id = match require_string(&input, "board_id") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let column_id = match require_string(&input, "column_id") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let title_raw = match require_string(&input, "title") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let title = match validate_task_title(&title_raw) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let description = optional_string(&input, "description");
        let status = optional_string(&input, "status");
        if let Err(e) = check_optional_status(status.as_deref()) {
            return e;
        }

        // Workspace check on the board.
        if BoardStore::get_in_workspace(&self.db, &board_id, &ctx.workspace_id).is_err() {
            return error(format!("Not found: board {board_id}"));
        }

        match TaskStore::create(
            &self.db,
            &board_id,
            &column_id,
            title,
            description.as_deref(),
            None,
            status.as_deref(),
        ) {
            Ok(task) => success(json!({"task": task})),
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
    async fn test_create_card_success() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateCardTool { db };
        let result = tool
            .execute(
                json!({
                    "board_id": bid,
                    "column_id": cid,
                    "title": "Implement auth",
                    "description": "Add login + logout endpoints"
                }),
                &ctx,
            )
            .await;

        assert!(result.success, "got: {:?}", result.error);
        let task = &result.data["task"];
        assert_eq!(task["title"], "Implement auth");
        assert_eq!(task["description"], "Add login + logout endpoints");
        assert_eq!(task["board_id"], bid);
        assert_eq!(task["column_id"], cid);
        assert_eq!(task["status"], "active");
    }

    #[tokio::test]
    async fn test_create_card_missing_title() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateCardTool { db };
        let result = tool
            .execute(json!({"board_id": bid, "column_id": cid}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_create_card_empty_title() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateCardTool { db };
        let result = tool
            .execute(
                json!({"board_id": bid, "column_id": cid, "title": "   "}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("empty"));
    }

    #[tokio::test]
    async fn test_create_card_title_too_long() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateCardTool { db };
        let result = tool
            .execute(
                json!({
                    "board_id": bid,
                    "column_id": cid,
                    "title": "a".repeat(501)
                }),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("500"));
    }

    #[tokio::test]
    async fn test_create_card_invalid_status() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateCardTool { db };
        let result = tool
            .execute(
                json!({
                    "board_id": bid,
                    "column_id": cid,
                    "title": "Card",
                    "status": "invalid"
                }),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("invalid task status"));
    }

    #[tokio::test]
    async fn test_create_card_cross_workspace_board() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let now = chrono::Utc::now().to_rfc3339();
        let other_ws = "other-workspace";
        let bid = uuid::Uuid::new_v4().to_string();
        let cid = uuid::Uuid::new_v4().to_string();
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
                 VALUES (?1, ?2, 'foreign', ?3)",
                rusqlite::params![bid, other_ws, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO columns (id, board_id, name, position, created_at)
                 VALUES (?1, ?2, 'col', 0, ?3)",
                rusqlite::params![cid, bid, now],
            )
            .unwrap();

        let tool = CreateCardTool { db };
        let result = tool
            .execute(
                json!({"board_id": bid, "column_id": cid, "title": "Card"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_create_card_column_from_other_board() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);
        // Add a second board in the same workspace.
        let other_board = uuid::Uuid::new_v4().to_string();
        let other_col = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at)
                 VALUES (?1, ?2, 'other-board', ?3)",
                rusqlite::params![other_board, ws, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO columns (id, board_id, name, position, created_at)
                 VALUES (?1, ?2, 'col', 0, ?3)",
                rusqlite::params![other_col, other_board, now],
            )
            .unwrap();

        let tool = CreateCardTool { db };
        let result = tool
            .execute(
                json!({
                    "board_id": bid,
                    "column_id": other_col,
                    "title": "Card"
                }),
                &ctx,
            )
            .await;

        assert!(!result.success, "expected column-board mismatch to fail");
        assert!(result.error.unwrap().contains("does not belong"));
    }

    #[tokio::test]
    async fn test_create_card_nonexistent_board() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateCardTool { db };
        let result = tool
            .execute(
                json!({
                    "board_id": "00000000-0000-0000-0000-000000000000",
                    "column_id": cid,
                    "title": "Card"
                }),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Not found: board"));
    }

    #[tokio::test]
    async fn test_create_card_nonexistent_column() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateCardTool { db };
        let result = tool
            .execute(
                json!({
                    "board_id": bid,
                    "column_id": "00000000-0000-0000-0000-000000000000",
                    "title": "Card"
                }),
                &ctx,
            )
            .await;

        assert!(!result.success);
        // Error comes from TaskStore::create, which surfaces the
        // FK constraint failure as a validation/internal error.
        let err = result.error.unwrap();
        assert!(
            err.contains("column") || err.contains("not found") || err.contains("FOREIGN"),
            "got: {err}"
        );
    }
}
