//! `update_card` — update card-level fields (title, description, priority, labels).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::tasks::{TaskStore, UpdateTask};
use crate::tools::fs::{error, optional_string, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct UpdateCardTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for UpdateCardTool {
    fn name(&self) -> &str {
        "update_card"
    }

    fn description(&self) -> &str {
        "Update card-level metadata on a task: title, description, priority, and labels. \
         Only the provided fields are changed; omitted fields are left as-is. \
         Returns the updated card."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "card_id": {
                    "type": "string",
                    "description": "The card (task) ID to update."
                },
                "title": {
                    "type": "string",
                    "description": "New title. 1-500 characters after trim."
                },
                "description": {
                    "type": "string",
                    "description": "New description. Set to null to clear."
                },
                "priority": {
                    "type": "string",
                    "description": "Priority label (e.g. 'critical', 'high', 'medium', 'low'). Set to null to clear."
                },
                "labels": {
                    "type": "string",
                    "description": "JSON array of label strings, e.g. '[\"bug\", \"urgent\"]'. Set to null to clear."
                }
            },
            "required": ["card_id"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let card_id = match require_string(&input, "card_id") {
            Ok(s) => s,
            Err(e) => return e,
        };

        let title = optional_string(&input, "title");
        let description = input.get("description").map(|v| {
            if v.is_null() {
                Some(None)
            } else {
                v.as_str().map(|s| Some(s.to_string()))
            }
        });
        let priority = optional_string(&input, "priority");
        let labels = input.get("labels").map(|v| {
            if v.is_null() {
                Some(None)
            } else {
                v.as_str().map(|s| Some(s.to_string()))
            }
        });

        let fields = UpdateTask {
            title,
            description: description.flatten(),
            column_id: None,
            position: None,
            status: None,
            session_id: None,
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: None,
            priority,
            labels: labels.flatten(),
            scope: None,
            verification_commands: None,
            test_cases: None,
            codebase_id: None,
        };

        match TaskStore::update(&self.db, &card_id, &ctx.workspace_id, &fields) {
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
    use serde_json::json;
    use tempfile::TempDir;

    fn seed_task(db: &Db) -> (String, String) {
        let (ws, bid, cid) = seed_workspace_with_board(db);
        let task_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Original', 0, 'active', ?4, ?4)",
                rusqlite::params![task_id, bid, cid, now],
            )
            .unwrap();
        (ws, task_id)
    }

    #[tokio::test]
    async fn test_update_card_title() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = UpdateCardTool { db };
        let r = tool
            .execute(json!({"card_id": task_id, "title": "Updated"}), &ctx)
            .await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["task"]["title"], "Updated");
    }

    #[tokio::test]
    async fn test_update_card_description_null_clears() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        // Set description first.
        db.conn()
            .execute(
                "UPDATE tasks SET description = 'body' WHERE id = ?1",
                [task_id.as_str()],
            )
            .unwrap();

        let tool = UpdateCardTool { db };
        let r = tool
            .execute(json!({"card_id": task_id, "description": null}), &ctx)
            .await;
        assert!(r.success, "got: {:?}", r.error);
        assert!(r.data["task"]["description"].is_null());
    }

    #[tokio::test]
    async fn test_update_card_priority_and_labels() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = UpdateCardTool { db };
        let r = tool
            .execute(
                json!({
                    "card_id": task_id,
                    "priority": "high",
                    "labels": "[\"bug\", \"urgent\"]"
                }),
                &ctx,
            )
            .await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["task"]["priority"], "high");
        assert_eq!(r.data["task"]["labels"], "[\"bug\", \"urgent\"]");
    }

    #[tokio::test]
    async fn test_update_card_no_fields_is_noop() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = UpdateCardTool { db };
        let r = tool.execute(json!({"card_id": task_id}), &ctx).await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["task"]["title"], "Original");
    }

    #[tokio::test]
    async fn test_update_card_not_found() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = UpdateCardTool { db };
        let r = tool.execute(json!({"card_id": "nonexistent"}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_update_card_cross_workspace() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (_ws, task_id) = seed_task(&db);
        let ctx = make_context(tmp.path()); // different workspace

        let tool = UpdateCardTool { db };
        let r = tool
            .execute(json!({"card_id": task_id, "title": "No"}), &ctx)
            .await;
        assert!(!r.success, "cross-workspace must be rejected");
        assert!(r.error.unwrap().contains("Not found"));
    }
}
