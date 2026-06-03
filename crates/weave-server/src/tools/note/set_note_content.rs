//! `set_note_content` — replace a note's content (bumps `updated_at`).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::notes::NoteStore;
use crate::tools::fs::{error, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct SetNoteContentTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for SetNoteContentTool {
    fn name(&self) -> &str {
        "set_note_content"
    }

    fn description(&self) -> &str {
        "Replace a note's content in full. The note's `id` and `created_at` \
         are preserved; `updated_at` is bumped to now. To append instead of \
         replace, use `append_to_note`. Cross-workspace access returns \
         NotFound."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The note ID to update."
                },
                "content": {
                    "type": "string",
                    "description": "The new content (replaces any existing content). Empty string is allowed."
                }
            },
            "required": ["id", "content"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let id = match require_string(&input, "id") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let content = match require_string(&input, "content") {
            Ok(s) => s,
            Err(e) => return e,
        };

        match NoteStore::set_content(&self.db, &id, &ctx.workspace_id, &content) {
            Ok(note) => success(json!({ "note": note })),
            Err(e) => error(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::{make_test_db, seed_workspace_with_board};
    use crate::tools::note::CreateNoteTool;
    use crate::tools::test_support::{make_context, make_context_for_workspace};
    use crate::tools::ToolExecutor;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_set_note_content_replaces() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let create = CreateNoteTool { db: db.clone() };
        let r = create
            .execute(
                json!({"title": "t", "type": "general", "content": "v1"}),
                &ctx,
            )
            .await;
        let id = r.data["note"]["id"].as_str().unwrap().to_string();

        let tool = SetNoteContentTool { db };
        let r = tool.execute(json!({"id": id, "content": "v2"}), &ctx).await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["note"]["content"], "v2");
    }

    #[tokio::test]
    async fn test_set_note_content_unknown_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = SetNoteContentTool { db };
        let r = tool
            .execute(json!({"id": "nope", "content": "x"}), &ctx)
            .await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_set_note_content_cross_workspace_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let create_ctx = make_context_for_workspace(tmp.path(), &ws);
        let create = CreateNoteTool { db: db.clone() };
        let r = create
            .execute(json!({"title": "t", "type": "general"}), &create_ctx)
            .await;
        let id = r.data["note"]["id"].as_str().unwrap().to_string();
        // Set from a different workspace.
        let set_ctx = make_context(tmp.path());
        let tool = SetNoteContentTool { db };
        let r = tool
            .execute(json!({"id": id, "content": "x"}), &set_ctx)
            .await;
        assert!(!r.success, "cross-workspace set must be rejected");
        assert!(r.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_set_note_content_missing_id() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = SetNoteContentTool { db };
        let r = tool.execute(json!({"content": "x"}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_set_note_content_missing_content() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = SetNoteContentTool { db };
        let r = tool.execute(json!({"id": "x"}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Missing"));
    }
}
