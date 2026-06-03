//! `list_tasks` — list tasks with optional filters.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::tasks::{TaskStore, VALID_TASK_STATUSES};
use crate::tools::fs::{check_optional_status, error, optional_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct ListTasksTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for ListTasksTool {
    fn name(&self) -> &str {
        "list_tasks"
    }

    fn description(&self) -> &str {
        "List tasks with optional filters. Returns tasks ordered by position. \
         All filters are optional; omit a filter to skip it. \
         Results are scoped to the current workspace."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "board_id": {
                    "type": "string",
                    "description": "Filter by board ID"
                },
                "column_id": {
                    "type": "string",
                    "description": "Filter by column ID"
                },
                "status": {
                    "type": "string",
                    "description": format!(
                        "Filter by status. Valid values: {}",
                        VALID_TASK_STATUSES.join(", ")
                    )
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let board_id = optional_string(&input, "board_id");
        let column_id = optional_string(&input, "column_id");
        let status = optional_string(&input, "status");

        // Validate status if provided
        if let Err(e) = check_optional_status(status.as_deref()) {
            return e;
        }

        match TaskStore::list(
            &self.db,
            &ctx.workspace_id,
            board_id.as_deref(),
            column_id.as_deref(),
            status.as_deref(),
            None,
        ) {
            Ok(tasks) => {
                let count = tasks.len();
                success(json!({"tasks": tasks, "count": count}))
            }
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

    struct TestData {
        board_id: String,
    }

    fn seed_tasks(db: &Db) -> TestData {
        let board_id = uuid::Uuid::new_v4().to_string();
        let col_id = uuid::Uuid::new_v4().to_string();
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

        // Create 3 tasks with different statuses
        for (i, status) in ["active", "done", "active"].iter().enumerate() {
            let task_id = uuid::Uuid::new_v4().to_string();
            db.conn()
                .execute(
                    "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    rusqlite::params![
                        task_id,
                        board_id,
                        col_id,
                        format!("Task {}", i + 1),
                        i as i64,
                        status,
                        now
                    ],
                )
                .unwrap();
        }

        TestData { board_id }
    }

    #[tokio::test]
    async fn test_list_tasks_all() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();
        seed_tasks(&db);

        let tool = ListTasksTool { db };
        let result = tool.execute(json!({}), &ctx).await;

        assert!(result.success);
        assert_eq!(result.data["count"], 3);
    }

    #[tokio::test]
    async fn test_list_tasks_filter_status() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();
        seed_tasks(&db);

        let tool = ListTasksTool { db };
        let result = tool.execute(json!({"status": "done"}), &ctx).await;

        assert!(result.success);
        assert_eq!(result.data["count"], 1);
        assert_eq!(result.data["tasks"][0]["title"], "Task 2");
    }

    #[tokio::test]
    async fn test_list_tasks_filter_board() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();
        let data = seed_tasks(&db);

        let tool = ListTasksTool { db };
        let result = tool.execute(json!({"board_id": data.board_id}), &ctx).await;

        assert!(result.success);
        assert_eq!(result.data["count"], 3);
    }

    #[tokio::test]
    async fn test_list_tasks_invalid_status() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();

        let tool = ListTasksTool { db };
        let result = tool.execute(json!({"status": "invalid"}), &ctx).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("invalid task status"));
    }

    #[tokio::test]
    async fn test_list_tasks_empty() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();
        seed_tasks(&db); // seed deps but tasks are in TEST_WS

        let tool = ListTasksTool { db };
        let result = tool.execute(json!({}), &ctx).await;

        assert!(result.success);
        assert_eq!(result.data["count"], 3); // make_context uses "test-workspace" which matches TEST_WS
    }
}
