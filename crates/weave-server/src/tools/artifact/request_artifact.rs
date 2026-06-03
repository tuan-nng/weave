//! `request_artifact` — declare intent to attach an artifact by `(task_id, type)`.
//!
//! Creates a fresh row with the given `content` (typically empty
//! for "I will attach later" semantics) and returns the row id.
//! If a row already exists for that `(task_id, type)`, returns
//! `Conflict` — use `provide_artifact` to amend.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::artifacts::ArtifactStore;
use crate::tools::fs::{error, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct RequestArtifactTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for RequestArtifactTool {
    fn name(&self) -> &str {
        "request_artifact"
    }

    fn description(&self) -> &str {
        "Create a new artifact on a task. The artifact is identified by \
         (task_id, type) — calling this twice with the same pair returns \
         Conflict. To amend an existing artifact's content, use \
         provide_artifact instead. Use `content` to attach the actual \
         evidence at creation time; leave it empty to declare intent \
         and fill in later."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task ID to attach the artifact to. Must belong to the current workspace."
                },
                "type": {
                    "type": "string",
                    "description": "Free-vocabulary artifact type. Common values: screenshot, test_results, code_diff, logs. The transition gate compares this string verbatim against the column's `required_artifact_types`."
                },
                "content": {
                    "type": "string",
                    "description": "Optional. The artifact's content. Empty string is allowed (the row exists; provide_artifact can fill it later)."
                }
            },
            "required": ["task_id", "type"]
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
        let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");

        match ArtifactStore::create(
            &self.db,
            &task_id,
            &artifact_type,
            content,
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

    /// Seed: workspace → board → column → task. Returns
    /// `(workspace_id, task_id)` so the test can build a matching
    /// `ToolContext` (the seeded workspace is the "default" one from
    /// `WorkspaceStore::ensure_default`, which is what
    /// `make_context_for_workspace(ws)` expects).
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
    async fn test_request_artifact_success() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = RequestArtifactTool { db };
        let r = tool
            .execute(
                json!({"task_id": task_id, "type": "screenshot", "content": "img"}),
                &ctx,
            )
            .await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["artifact"]["type"], "screenshot");
        assert_eq!(r.data["artifact"]["content"], "img");
        assert_eq!(r.data["artifact"]["task_id"], task_id);
    }

    #[tokio::test]
    async fn test_request_artifact_default_content_is_empty() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = RequestArtifactTool { db };
        let r = tool
            .execute(json!({"task_id": task_id, "type": "log"}), &ctx)
            .await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["artifact"]["content"], "");
    }

    #[tokio::test]
    async fn test_request_artifact_duplicate_type_returns_conflict() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, task_id) = seed_task(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = RequestArtifactTool { db };
        let r1 = tool
            .execute(
                json!({"task_id": task_id, "type": "screenshot", "content": "first"}),
                &ctx,
            )
            .await;
        assert!(r1.success, "first call: {:?}", r1.error);
        let r2 = tool
            .execute(
                json!({"task_id": task_id, "type": "screenshot", "content": "second"}),
                &ctx,
            )
            .await;
        assert!(
            !r2.success,
            "duplicate (task_id, type) must return Conflict"
        );
        assert!(r2.error.unwrap().contains("already exists"));
    }

    #[tokio::test]
    async fn test_request_artifact_unknown_task_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = RequestArtifactTool { db };
        let r = tool
            .execute(
                json!({"task_id": "no-such-task", "type": "screenshot"}),
                &ctx,
            )
            .await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_request_artifact_missing_task_id() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = RequestArtifactTool { db };
        let r = tool.execute(json!({"type": "screenshot"}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_request_artifact_missing_type() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = RequestArtifactTool { db };
        let r = tool.execute(json!({"task_id": "x"}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_request_artifact_cross_workspace_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        // Seed a task in the "default" workspace (returned by
        // `seed_workspace_with_board`), then use a test-context that
        // points at a different workspace ("test-workspace"). The
        // workspace_id mismatch alone is enough to prove the
        // cross-workspace defense works — no need to manually
        // construct a foreign-workspace FK chain.
        let db = make_test_db();
        let (_ws, task_id) = seed_task(&db);
        let ctx = make_context(tmp.path()); // workspace_id = "test-workspace"

        let tool = RequestArtifactTool { db };
        let r = tool
            .execute(json!({"task_id": task_id, "type": "log"}), &ctx)
            .await;
        assert!(!r.success, "cross-workspace request must be rejected");
        assert!(r.error.unwrap().contains("Not found"));
    }
}
