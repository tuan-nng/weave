//! `read_note` — fetch one note by id, workspace-scoped.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::notes::NoteStore;
use crate::tools::fs::{error, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct ReadNoteTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for ReadNoteTool {
    fn name(&self) -> &str {
        "read_note"
    }

    fn description(&self) -> &str {
        "Read a single note by its id. Workspace-scoped: a note in a \
         different workspace returns NotFound, not Forbidden. Use this \
         to pull the full content of a note discovered via list_notes."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The note ID (UUID)."
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let id = match require_string(&input, "id") {
            Ok(s) => s,
            Err(e) => return e,
        };

        match NoteStore::get_by_id(&self.db, &id, &ctx.workspace_id) {
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
    async fn test_read_note_returns_row() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let create = CreateNoteTool { db: db.clone() };
        let r = create
            .execute(
                json!({"title": "t", "type": "general", "content": "hello"}),
                &ctx,
            )
            .await;
        let id = r.data["note"]["id"].as_str().unwrap().to_string();

        let read = ReadNoteTool { db };
        let r = read.execute(json!({"id": id}), &ctx).await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["note"]["id"], id);
        assert_eq!(r.data["note"]["content"], "hello");
    }

    #[tokio::test]
    async fn test_read_note_unknown_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = ReadNoteTool { db };
        let r = tool.execute(json!({"id": "nope"}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_read_note_cross_workspace_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        // Seed in the "default" workspace.
        let create_ctx = make_context_for_workspace(tmp.path(), &ws);
        let create = CreateNoteTool { db: db.clone() };
        let r = create
            .execute(json!({"title": "t", "type": "general"}), &create_ctx)
            .await;
        let id = r.data["note"]["id"].as_str().unwrap().to_string();
        // Read from a different workspace.
        let read_ctx = make_context(tmp.path()); // "test-workspace"
        let read = ReadNoteTool { db };
        let r = read.execute(json!({"id": id}), &read_ctx).await;
        assert!(!r.success, "cross-workspace read must be rejected");
        assert!(r.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_read_note_missing_id() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let tool = ReadNoteTool { db };
        let r = tool.execute(json!({}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("Missing"));
    }
}
