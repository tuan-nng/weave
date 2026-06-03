//! `provide_artifact` — upsert artifact content by `(task_id, type)`.
//!
//! If a row exists, `content` is replaced and `updated_at` is bumped;
//! `id` and `created_at` are preserved. If not, a new row is inserted.
//! Returns the row id either way. Workspace-scoped.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::artifacts::ArtifactStore;
use crate::tools::fs::{error, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct ProvideArtifactTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for ProvideArtifactTool {
    fn name(&self) -> &str {
        "provide_artifact"
    }

    fn description(&self) -> &str {
        "Attach or update an artifact's content. Identified by (task_id, type) — \
         if a row with that pair already exists, its `content` is replaced; \
         otherwise a new row is created. Returns the row id either way. \
         This is the canonical way to attach evidence before moving a card \
         into a column with `required_artifact_types` set."
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
                    "description": "Free-vocabulary artifact type. Common values: screenshot, test_results, code_diff, logs."
                },
                "content": {
                    "type": "string",
                    "description": "The artifact's content. Replaces any prior content for this (task_id, type) pair."
                }
            },
            "required": ["task_id", "type", "content"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let task_id = match require_string(&input, "task_id") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let artifact_type = match require_string(&input, "type") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let content = match require_string(&input, "content") {
            Ok(s) => s,
            Err(e) => return e,
        };

        match ArtifactStore::provide(
            &self.db,
            &task_id,
            &artifact_type,
            &content,
            &ctx.workspace_id,
        ) {
            Ok(artifact) => success(json!({ "artifact": artifact })),
            Err(e) => error(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
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
    async fn test_provide_artifact_creates_when_absent() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = ProvideArtifactTool { db };
        let r = tool
            .execute(
                json!({"task_id": task_id, "type": "log", "content": "line 1"}),
                &ctx,
            )
            .await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["artifact"]["content"], "line 1");
        assert!(!r.data["artifact"]["id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_provide_artifact_updates_when_present_preserves_id() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = ProvideArtifactTool { db };
        let r1 = tool
            .execute(
                json!({"task_id": task_id, "type": "log", "content": "v1"}),
                &ctx,
            )
            .await;
        assert!(r1.success);
        let first_id = r1.data["artifact"]["id"].as_str().unwrap().to_string();
        let first_created = r1.data["artifact"]["created_at"]
            .as_str()
            .unwrap()
            .to_string();

        let r2 = tool
            .execute(
                json!({"task_id": task_id, "type": "log", "content": "v2"}),
                &ctx,
            )
            .await;
        assert!(r2.success);
        let second_id = r2.data["artifact"]["id"].as_str().unwrap().to_string();
        let second_created = r2.data["artifact"]["created_at"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(first_id, second_id, "upsert preserves id");
        assert_eq!(first_created, second_created, "upsert preserves created_at");
        assert_eq!(r2.data["artifact"]["content"], "v2");
    }

    #[tokio::test]
    async fn test_provide_artifact_does_not_touch_other_types() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = ProvideArtifactTool { db };
        let r1 = tool
            .execute(
                json!({"task_id": task_id, "type": "log", "content": "a"}),
                &ctx,
            )
            .await;
        assert!(r1.success, "first provide: {:?}", r1.error);
        let r2 = tool
            .execute(
                json!({"task_id": task_id, "type": "screenshot", "content": "img"}),
                &ctx,
            )
            .await;
        assert!(r2.success, "second provide: {:?}", r2.error);
        // Upsert on log must not affect screenshot.
        let r3 = tool
            .execute(
                json!({"task_id": task_id, "type": "log", "content": "a-v2"}),
                &ctx,
            )
            .await;
        assert!(r3.success, "third provide: {:?}", r3.error);
        let list =
            crate::store::artifacts::ArtifactStore::list_by_task(&tool.db, &task_id, &ws, None)
                .unwrap();
        assert_eq!(list.len(), 2);
        let log = list.iter().find(|a| a.type_ == "log").unwrap();
        assert_eq!(log.content, "a-v2");
        let screenshot = list.iter().find(|a| a.type_ == "screenshot").unwrap();
        assert_eq!(screenshot.content, "img");
    }

    #[tokio::test]
    async fn test_provide_artifact_unknown_task_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = ProvideArtifactTool { db };
        let r = tool
            .execute(
                json!({"task_id": "no-such", "type": "log", "content": "x"}),
                &ctx,
            )
            .await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_provide_artifact_missing_field() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = ProvideArtifactTool { db };
        // Missing content
        let r = tool
            .execute(json!({"task_id": "x", "type": "log"}), &ctx)
            .await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Missing"));
        // Missing type
        let r = tool
            .execute(json!({"task_id": "x", "content": "x"}), &ctx)
            .await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Missing"));
        // Missing task_id
        let r = tool
            .execute(json!({"type": "log", "content": "x"}), &ctx)
            .await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_provide_artifact_cross_workspace_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        // Seed a task in the "default" workspace (returned by
        // `seed_workspace_with_board`), then use a test-context that
        // points at a different workspace ("test-workspace"). The
        // workspace_id mismatch alone is enough to prove the
        // cross-workspace defense works — no need to manually
        // construct a foreign-workspace FK chain.
        let ctx = make_context(tmp.path()); // test-workspace
        let db = make_test_db();
        let (_ws, task_id) = seed_task(&db);

        let tool = ProvideArtifactTool { db };
        let r = tool
            .execute(
                json!({"task_id": task_id, "type": "log", "content": "x"}),
                &ctx,
            )
            .await;
        assert!(!r.success, "cross-workspace must be rejected");
        assert!(r.error.unwrap().contains("Not found"));
    }
}
