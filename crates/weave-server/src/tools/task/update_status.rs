//! `update_task_status` — transition a task's status.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::tasks::{TaskStore, VALID_TASK_STATUSES};
use crate::tools::fs::{error, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct UpdateTaskStatusTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for UpdateTaskStatusTool {
    fn name(&self) -> &str {
        "update_task_status"
    }

    fn description(&self) -> &str {
        "Update a task's status. Valid transitions: in_progress, review_required, \
         completed, needs_fix, blocked."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task ID to update"
                },
                "status": {
                    "type": "string",
                    "description": format!(
                        "New status value. Valid values: {}",
                        VALID_TASK_STATUSES.join(", ")
                    )
                }
            },
            "required": ["task_id", "status"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let task_id = match require_string(&input, "task_id") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let status = match require_string(&input, "status") {
            Ok(s) => s,
            Err(e) => return e,
        };

        match TaskStore::update_status(&self.db, &task_id, &ctx.workspace_id, &status) {
            Ok(task) => success(json!({"task": task})),
            Err(e) => error(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::make_context;
    use chrono::Utc;
    use std::path::Path;
    use tempfile::TempDir;

    const TEST_WS: &str = "test-workspace";

    fn test_db() -> Arc<Db> {
        Arc::new(Db::open(Path::new(":memory:")).expect("failed to open test db"))
    }

    fn seed_task(db: &Db) -> String {
        let board_id = uuid::Uuid::new_v4().to_string();
        let col_id = uuid::Uuid::new_v4().to_string();
        let task_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'ws', 'active', ?2, ?2)",
                rusqlite::params![TEST_WS, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at)
                 VALUES (?1, ?2, 'board', ?3)",
                rusqlite::params![board_id, TEST_WS, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO columns (id, board_id, name, position, created_at)
                 VALUES (?1, ?2, 'col', 0, ?3)",
                rusqlite::params![col_id, board_id, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Test Task', 0, 'in_progress', ?4, ?4)",
                rusqlite::params![task_id, board_id, col_id, now],
            )
            .unwrap();

        task_id
    }

    #[tokio::test]
    async fn test_update_task_status_success() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();
        let task_id = seed_task(&db);

        let tool = UpdateTaskStatusTool { db };
        let result = tool
            .execute(json!({"task_id": task_id, "status": "completed"}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["task"]["status"], "completed");
    }

    #[tokio::test]
    async fn test_update_task_status_invalid() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();
        let task_id = seed_task(&db);

        let tool = UpdateTaskStatusTool { db };
        let result = tool
            .execute(json!({"task_id": task_id, "status": "invalid"}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("invalid task status"));
    }

    #[tokio::test]
    async fn test_update_task_status_missing_fields() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();
        seed_task(&db);

        let tool = UpdateTaskStatusTool { db };

        // Missing task_id
        let result = tool.execute(json!({"status": "completed"}), &ctx).await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("Missing"));

        // Missing status
        let tool = UpdateTaskStatusTool { db: test_db() };
        let result = tool.execute(json!({"task_id": "abc"}), &ctx).await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_update_task_status_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();

        let tool = UpdateTaskStatusTool { db };
        let result = tool
            .execute(
                json!({"task_id": "nonexistent", "status": "completed"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_update_task_status() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();
        let task_id = seed_task(&db);

        let tool = UpdateTaskStatusTool { db };
        let result = tool
            .execute(json!({"task_id": task_id, "status": "completed"}), &ctx)
            .await;
        assert!(result.success);
        assert_eq!(result.data["task"]["status"], "completed");
    }
}
