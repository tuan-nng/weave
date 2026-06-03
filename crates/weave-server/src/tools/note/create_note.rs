//! `create_note` — create a new workspace note by `(title, type, content)`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::notes::NoteStore;
use crate::tools::fs::{error, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct CreateNoteTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for CreateNoteTool {
    fn name(&self) -> &str {
        "create_note"
    }

    fn description(&self) -> &str {
        "Create a new note in the current workspace. The (workspace, title) pair \
         is unique — calling this twice with the same title returns Conflict. \
         Use `type` to categorize the note (one of: spec, task, general). \
         Use `content` to attach the initial text; leave it empty to declare \
         intent and fill in later with set_note_content or append_to_note."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "The note title. Must be unique within the workspace. Trimmed of surrounding whitespace; max 200 chars."
                },
                "type": {
                    "type": "string",
                    "description": "The note type. One of: spec, task, general."
                },
                "content": {
                    "type": "string",
                    "description": "Optional. The note's initial content. Empty string is allowed (the row exists; set_note_content or append_to_note can fill it later)."
                }
            },
            "required": ["title", "type"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let title = match require_string(&input, "title") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let note_type = match require_string(&input, "type") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");

        match NoteStore::create(&self.db, &ctx.workspace_id, &title, &note_type, content) {
            Ok(note) => success(json!({ "note": note })),
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

    #[tokio::test]
    async fn test_create_note_success() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateNoteTool { db };
        let r = tool
            .execute(
                json!({"title": "API contract", "type": "spec", "content": "v1"}),
                &ctx,
            )
            .await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["note"]["title"], "API contract");
        assert_eq!(r.data["note"]["type"], "spec");
        assert_eq!(r.data["note"]["content"], "v1");
    }

    #[tokio::test]
    async fn test_create_note_default_content_is_empty() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateNoteTool { db };
        let r = tool
            .execute(json!({"title": "stub", "type": "general"}), &ctx)
            .await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["note"]["content"], "");
    }

    #[tokio::test]
    async fn test_create_note_duplicate_title_returns_conflict() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateNoteTool { db };
        let r1 = tool
            .execute(json!({"title": "t", "type": "spec", "content": "v1"}), &ctx)
            .await;
        assert!(r1.success, "first: {:?}", r1.error);
        let r2 = tool
            .execute(
                json!({"title": "t", "type": "general", "content": "v2"}),
                &ctx,
            )
            .await;
        assert!(!r2.success, "duplicate (ws, title) must return Conflict");
        assert!(r2.error.unwrap().contains("already exists"));
    }

    #[tokio::test]
    async fn test_create_note_invalid_type_returns_validation() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateNoteTool { db };
        let r = tool
            .execute(json!({"title": "t", "type": "freeform"}), &ctx)
            .await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("invalid note type"));
    }

    #[tokio::test]
    async fn test_create_note_empty_title_returns_validation() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = CreateNoteTool { db };
        let r = tool
            .execute(json!({"title": "   ", "type": "general"}), &ctx)
            .await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("must not be empty"));
    }

    #[tokio::test]
    async fn test_create_note_missing_title() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = CreateNoteTool { db };
        let r = tool.execute(json!({"type": "general"}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_create_note_missing_type() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = CreateNoteTool { db };
        let r = tool.execute(json!({"title": "t"}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Missing"));
    }
}
