//! `update_task` — update task-level fields (scope, acceptance_criteria,
//! verification_commands, test_cases).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::tasks::{TaskStore, UpdateTask};
use crate::tools::fs::{error, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct UpdateTaskTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for UpdateTaskTool {
    fn name(&self) -> &str {
        "update_task"
    }

    fn description(&self) -> &str {
        "Update task-level structured fields: scope, acceptance_criteria, \
         verification_commands, and test_cases. Only the provided fields are \
         changed; omitted fields are left as-is. Returns the updated task."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task ID to update."
                },
                "scope": {
                    "type": "string",
                    "description": "Task scope description. Set to null to clear."
                },
                "acceptance_criteria": {
                    "type": "string",
                    "description": "Acceptance criteria for the task. Set to null to clear."
                },
                "verification_commands": {
                    "type": "string",
                    "description": "Commands to verify the task is done (e.g. 'cargo test'). Set to null to clear."
                },
                "test_cases": {
                    "type": "string",
                    "description": "Test cases or expected behaviors. Set to null to clear."
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

        let scope = input.get("scope").map(|v| {
            if v.is_null() {
                Some(None)
            } else {
                v.as_str().map(|s| Some(s.to_string()))
            }
        });
        let acceptance_criteria = input.get("acceptance_criteria").map(|v| {
            if v.is_null() {
                Some(None)
            } else {
                v.as_str().map(|s| Some(s.to_string()))
            }
        });
        let verification_commands = input.get("verification_commands").map(|v| {
            if v.is_null() {
                Some(None)
            } else {
                v.as_str().map(|s| Some(s.to_string()))
            }
        });
        let test_cases = input.get("test_cases").map(|v| {
            if v.is_null() {
                Some(None)
            } else {
                v.as_str().map(|s| Some(s.to_string()))
            }
        });

        let fields = UpdateTask {
            title: None,
            description: None,
            column_id: None,
            position: None,
            status: None,
            session_id: None,
            acceptance_criteria: acceptance_criteria.flatten(),
            completion_summary: None,
            verification_report: None,
            priority: None,
            labels: None,
            scope: scope.flatten(),
            verification_commands: verification_commands.flatten(),
            test_cases: test_cases.flatten(),
            codebase_id: None,
        };

        match TaskStore::update(&self.db, &task_id, &ctx.workspace_id, &fields) {
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
    use crate::tools::ToolExecutor;
    use tempfile::TempDir;

    fn seed_task(db: &Db) -> (String, String) {
        let (ws, bid, cid) = seed_workspace_with_board(db);
        let task_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'T', 0, 'active', ?4, ?4)",
                rusqlite::params![task_id, bid, cid, now],
            )
            .unwrap();
        (ws, task_id)
    }

    #[tokio::test]
    async fn test_update_task_acceptance_criteria() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = UpdateTaskTool { db };
        let r = tool
            .execute(
                json!({"task_id": task_id, "acceptance_criteria": "All tests pass"}),
                &ctx,
            )
            .await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["task"]["acceptance_criteria"], "All tests pass");
    }

    #[tokio::test]
    async fn test_update_task_scope_and_test_cases() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = UpdateTaskTool { db };
        let r = tool
            .execute(
                json!({
                    "task_id": task_id,
                    "scope": "Backend only",
                    "test_cases": "unit + integration"
                }),
                &ctx,
            )
            .await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["task"]["scope"], "Backend only");
        assert_eq!(r.data["task"]["test_cases"], "unit + integration");
    }

    #[tokio::test]
    async fn test_update_task_null_clears() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        // Set a value first.
        db.conn()
            .execute(
                "UPDATE tasks SET acceptance_criteria = 'AC' WHERE id = ?1",
                [task_id.as_str()],
            )
            .unwrap();

        let tool = UpdateTaskTool { db };
        let r = tool
            .execute(
                json!({"task_id": task_id, "acceptance_criteria": null}),
                &ctx,
            )
            .await;
        assert!(r.success, "got: {:?}", r.error);
        assert!(r.data["task"]["acceptance_criteria"].is_null());
    }

    #[tokio::test]
    async fn test_update_task_not_found() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = UpdateTaskTool { db };
        let r = tool.execute(json!({"task_id": "nonexistent"}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_update_task_cross_workspace() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (_ws, task_id) = seed_task(&db);
        let ctx = make_context(tmp.path()); // different workspace

        let tool = UpdateTaskTool { db };
        let r = tool
            .execute(json!({"task_id": task_id, "scope": "No"}), &ctx)
            .await;
        assert!(!r.success, "cross-workspace must be rejected");
        assert!(r.error.unwrap().contains("Not found"));
    }
}
