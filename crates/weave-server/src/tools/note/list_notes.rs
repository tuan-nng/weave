//! `list_notes` — workspace-scoped list of notes with optional type filter.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::store::notes::NoteStore;
use crate::tools::fs::{error, optional_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct ListNotesTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for ListNotesTool {
    fn name(&self) -> &str {
        "list_notes"
    }

    fn description(&self) -> &str {
        "List notes in the current workspace, ordered by `updated_at` \
         DESC, `id` DESC (most-recently-touched first). Optionally filter \
         by `type` (one of: spec, task, general). Returns the matching \
         notes and a count. This is the canonical read-only entry point \
         for the notes surface."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "type": {
                    "type": "string",
                    "description": "Optional. Filter to a single note type (one of: spec, task, general)."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let type_filter = optional_string(&input, "type");

        match NoteStore::list(&self.db, &ctx.workspace_id, type_filter.as_deref()) {
            Ok(notes) => {
                let count = notes.len();
                success(json!({ "notes": notes, "count": count }))
            }
            Err(e) => error(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::{make_test_db, seed_workspace_with_board};
    use crate::tools::note::CreateNoteTool;
    use crate::tools::test_support::make_context_for_workspace;
    use crate::tools::ToolExecutor;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_list_notes_returns_all() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let create = CreateNoteTool { db: db.clone() };
        for (title, ty) in [("a", "spec"), ("b", "general"), ("c", "spec")] {
            let r = create
                .execute(json!({"title": title, "type": ty}), &ctx)
                .await;
            assert!(r.success, "seed {title}: {:?}", r.error);
        }

        let tool = ListNotesTool { db };
        let r = tool.execute(json!({}), &ctx).await;
        assert!(r.success, "got: {:?}", r.error);
        assert_eq!(r.data["count"], 3);
        assert!(r.data["notes"].is_array());
    }

    #[tokio::test]
    async fn test_list_notes_with_type_filter() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let create = CreateNoteTool { db: db.clone() };
        for (title, ty) in [("a", "spec"), ("b", "general"), ("c", "spec")] {
            let r = create
                .execute(json!({"title": title, "type": ty}), &ctx)
                .await;
            assert!(r.success, "seed {title}: {:?}", r.error);
        }

        let tool = ListNotesTool { db };
        let r = tool.execute(json!({"type": "spec"}), &ctx).await;
        assert!(r.success);
        assert_eq!(r.data["count"], 2);
        assert!(
            r.data["notes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|n| n["type"] == "spec"),
            "filter must narrow to specs only"
        );
    }

    #[tokio::test]
    async fn test_list_notes_invalid_type_filter_returns_validation() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = ListNotesTool { db };
        let r = tool.execute(json!({"type": "freeform"}), &ctx).await;
        assert!(!r.success);
        assert!(r.error.unwrap().contains("invalid note type"));
    }

    #[tokio::test]
    async fn test_list_notes_empty() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let tool = ListNotesTool { db };
        let r = tool.execute(json!({}), &ctx).await;
        assert!(r.success);
        assert_eq!(r.data["count"], 0);
    }
}
