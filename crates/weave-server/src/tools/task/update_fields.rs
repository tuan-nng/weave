//! `update_task_fields` — update completion_summary and verification_report.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::tasks::{TaskStore, UpdateTaskFields};
use crate::tools::fs::{error, optional_string, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct UpdateTaskFieldsTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for UpdateTaskFieldsTool {
    fn name(&self) -> &str {
        "update_task_fields"
    }

    fn description(&self) -> &str {
        "Update a task's context fields: completion_summary (what was done) and \
         verification_report (evidence of correctness). At least one field must be provided."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task ID to update"
                },
                "acceptance_criteria": {
                    "type": "string",
                    "description": "What 'done' looks like for this task"
                },
                "completion_summary": {
                    "type": "string",
                    "description": "Summary of what was done"
                },
                "verification_report": {
                    "type": "string",
                    "description": "Evidence that acceptance criteria are met"
                }
            },
            "required": ["task_id"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let task_id = match require_string(&input, "task_id") {
            Ok(s) => s,
            Err(e) => return e,
        };

        let acceptance_criteria = optional_string(&input, "acceptance_criteria");
        let completion_summary = optional_string(&input, "completion_summary");
        let verification_report = optional_string(&input, "verification_report");

        if acceptance_criteria.is_none()
            && completion_summary.is_none()
            && verification_report.is_none()
        {
            return error(
                "At least one of acceptance_criteria, completion_summary, or \
                 verification_report must be provided.",
            );
        }

        let fields = UpdateTaskFields {
            acceptance_criteria,
            completion_summary,
            verification_report,
        };

        match TaskStore::update_fields(&self.db, &task_id, &ctx.workspace_id, &fields) {
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
    async fn test_update_task_fields_success() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();
        let task_id = seed_task(&db);

        let tool = UpdateTaskFieldsTool { db };
        let result = tool
            .execute(
                json!({
                    "task_id": task_id,
                    "completion_summary": "Implemented feature X",
                    "verification_report": "All tests pass"
                }),
                &ctx,
            )
            .await;

        assert!(result.success);
        assert_eq!(
            result.data["task"]["completion_summary"],
            "Implemented feature X"
        );
        assert_eq!(result.data["task"]["verification_report"], "All tests pass");
    }

    #[tokio::test]
    async fn test_update_task_fields_no_fields() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();
        let task_id = seed_task(&db);

        let tool = UpdateTaskFieldsTool { db };
        let result = tool.execute(json!({"task_id": task_id}), &ctx).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("At least one"));
    }

    #[tokio::test]
    async fn test_update_task_fields_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();

        let tool = UpdateTaskFieldsTool { db };
        let result = tool
            .execute(
                json!({"task_id": "nonexistent", "completion_summary": "test"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_update_task_fields_missing_id() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = test_db();

        let tool = UpdateTaskFieldsTool { db };
        let result = tool
            .execute(json!({"completion_summary": "test"}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Missing"));
    }
}
