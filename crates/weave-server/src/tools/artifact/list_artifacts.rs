//! `list_artifacts` — workspace-scoped list of artifacts attached to a task.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::artifacts::ArtifactStore;
use crate::tools::fs::{error, optional_string, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct ListArtifactsTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for ListArtifactsTool {
    fn name(&self) -> &str {
        "list_artifacts"
    }

    fn description(&self) -> &str {
        "List artifacts attached to a task. Scoped to the current workspace. \
         Optionally filter by `type`. Returns the matching artifacts and a count. \
         This is the canonical read-only entry point for evidence attached to \
         a kanban task."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task ID. Must belong to the current workspace."
                },
                "type": {
                    "type": "string",
                    "description": "Optional. Filter to a single artifact type (e.g. 'screenshot', 'log')."
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
        let artifact_type = optional_string(&input, "type");

        match ArtifactStore::list_by_task(
            &self.db,
            &task_id,
            &ctx.workspace_id,
            artifact_type.as_deref(),
        ) {
            Ok(artifacts) => {
                let count = artifacts.len();
                success(json!({ "artifacts": artifacts, "count": count }))
            }
            Err(e) => error(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::store::artifacts::ArtifactStore;
    use crate::store::kanban_test_helpers::{make_test_db, seed_workspace_with_board};
    use crate::tools::test_support::{make_context, make_context_for_workspace};
    use crate::tools::ToolExecutor;
    use tempfile::TempDir;

    fn seed_task_with_artifacts(db: &Db) -> (String, String) {
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
        // Seed three artifacts of two types.
        ArtifactStore::create(db, &task_id, "screenshot", "img", &ws).unwrap();
        ArtifactStore::create(db, &task_id, "log", "v1", &ws).unwrap();
        ArtifactStore::create(db, &task_id, "test_results", "ok", &ws).unwrap();
        (ws, task_id)
    }

    #[tokio::test]
    async fn test_list_artifacts_returns_all() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task_with_artifacts(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = ListArtifactsTool { db };
        let r = tool.execute(json!({"task_id": task_id}), &ctx).await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["count"], 3);
        assert!(r.data["artifacts"].is_array());
    }

    #[tokio::test]
    async fn test_list_artifacts_with_type_filter() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task_with_artifacts(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = ListArtifactsTool { db };
        let r = tool
            .execute(json!({"task_id": task_id, "type": "screenshot"}), &ctx)
            .await;
        assert!(r.success);
        assert_eq!(r.data["count"], 1);
        assert_eq!(r.data["artifacts"][0]["type"], "screenshot");
    }

    #[tokio::test]
    async fn test_list_artifacts_no_match() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task_with_artifacts(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = ListArtifactsTool { db };
        let r = tool
            .execute(json!({"task_id": task_id, "type": "nonexistent"}), &ctx)
            .await;
        assert!(r.success);
        assert_eq!(r.data["count"], 0);
    }

    #[tokio::test]
    async fn test_list_artifacts_empty() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, cid) = seed_workspace_with_board(&db);
        let task_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'T', 0, 'active', ?4, ?4)",
                rusqlite::params![task_id, bid, cid, now],
            )
            .unwrap();
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = ListArtifactsTool { db };
        let r = tool.execute(json!({"task_id": task_id}), &ctx).await;
        assert!(r.success);
        assert_eq!(r.data["count"], 0);
    }

    #[tokio::test]
    async fn test_list_artifacts_missing_task_id() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = ListArtifactsTool { db };
        let r = tool.execute(json!({}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_list_artifacts_unknown_task_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = ListArtifactsTool { db };
        let r = tool.execute(json!({"task_id": "no-such"}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_list_artifacts_cross_workspace_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path()); // test-workspace
        let db = make_test_db();
        let (_ws, task_id) = seed_task_with_artifacts(&db);
        let tool = ListArtifactsTool { db };
        let r = tool.execute(json!({"task_id": task_id}), &ctx).await;
        // The seeded workspace is "default" (from seed_workspace_with_board),
        // which is NOT the test-workspace that `make_context` uses.
        // Cross-workspace defense must reject.
        assert!(!r.success, "cross-workspace list must be rejected");
        assert!(r.error.unwrap().contains("Not found"));
    }
}
