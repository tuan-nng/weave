//! Note tools for agents (feat-030).
//!
//! Five tool executors that give an agent autonomous access to the
//! workspace notes surface (long-form text kept between sessions):
//!
//! - `create_note` — create a new note by `(title, type, content)` in
//!   the current workspace. The `(workspace_id, title)` pair is
//!   UNIQUE — re-creating returns `Conflict`. Use `set_note_content`
//!   or `append_to_note` to amend an existing note.
//! - `read_note` — fetch one note by id, workspace-scoped.
//! - `list_notes` — workspace-scoped list of notes, with an optional
//!   `type` filter. Ordered `updated_at DESC, id DESC` so the
//!   most-recently-touched notes surface first.
//! - `set_note_content` — replace a note's content (bumps
//!   `updated_at`).
//! - `append_to_note` — append a suffix to a note's content in a
//!   single SQL statement (bumps `updated_at`).
//!
//! All five tools hold `Arc<Db>` (consistent with the other DB-backed
//! tools). They deliberately do NOT fire SSE events — the agent
//! invokes them at any time, independent of the chat/move path.

pub mod append_to_note;
pub mod create_note;
pub mod list_notes;
pub mod read_note;
pub mod set_note_content;

pub use append_to_note::AppendToNoteTool;
pub use create_note::CreateNoteTool;
pub use list_notes::ListNotesTool;
pub use read_note::ReadNoteTool;
pub use set_note_content::SetNoteContentTool;

#[cfg(test)]
mod tests {
    //! Verification-gate tests for feat-030.
    //!
    //! The two tests in this module are the named verification targets
    //! from `feature_list.json`:
    //!
    //! - `test_note_crud` — exercises all five tool executors in a
    //!   single fixture: `create_note`, `read_note`, `list_notes`,
    //!   `set_note_content`, `append_to_note`. Proves the tools are
    //!   registered, callable, and return the expected shapes.
    //! - `test_note_append` — focused append behavior: three appends
    //!   grow the content linearly and `updated_at` bumps on each
    //!   call.
    use super::*;
    use crate::store::kanban_test_helpers::{make_test_db, seed_workspace_with_board};
    use crate::tools::test_support::make_context_for_workspace;
    use crate::tools::ToolExecutor;
    use serde_json::json;
    use tempfile::TempDir;

    /// Verification gate 1: exercise all five note tools in one fixture.
    #[tokio::test]
    async fn test_note_crud() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        // 1. create_note — new spec note.
        let create = CreateNoteTool { db: db.clone() };
        let r = create
            .execute(
                json!({"title": "API contract", "type": "spec", "content": "v1"}),
                &ctx,
            )
            .await;
        assert!(r.success, "create_note: {:?}", r.error);
        let note_id = r.data["note"]["id"].as_str().unwrap().to_string();
        assert_eq!(r.data["note"]["type"], "spec");
        assert_eq!(r.data["note"]["title"], "API contract");
        assert_eq!(r.data["note"]["content"], "v1");

        // 2. read_note — sees the new row.
        let read = ReadNoteTool { db: db.clone() };
        let r = read.execute(json!({"id": note_id}), &ctx).await;
        assert!(r.success, "read_note: {:?}", r.error);
        assert_eq!(r.data["note"]["id"], note_id);

        // 3. list_notes — returns at least the one we just created.
        let list = ListNotesTool { db: db.clone() };
        let r = list.execute(json!({}), &ctx).await;
        assert!(r.success, "list_notes: {:?}", r.error);
        assert!(
            r.data["count"].as_u64().unwrap() >= 1,
            "list must include the new note"
        );

        // 4. set_note_content — replace content.
        let set_content = SetNoteContentTool { db: db.clone() };
        let r = set_content
            .execute(json!({"id": note_id, "content": "v2"}), &ctx)
            .await;
        assert!(r.success, "set_note_content: {:?}", r.error);
        assert_eq!(r.data["note"]["content"], "v2");

        // 5. append_to_note — append a suffix.
        let append = AppendToNoteTool { db: db.clone() };
        let r = append
            .execute(json!({"id": note_id, "content": "-tail"}), &ctx)
            .await;
        assert!(r.success, "append_to_note: {:?}", r.error);
        assert_eq!(r.data["note"]["content"], "v2-tail");

        // 6. list_notes with type filter — only the one we created.
        let r = list.execute(json!({"type": "spec"}), &ctx).await;
        assert!(r.success);
        let count = r.data["count"].as_u64().unwrap();
        assert!(count >= 1, "spec filter must include the note");
        assert!(
            r.data["notes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|n| n["type"] == "spec"),
            "type filter must narrow the list"
        );

        // 7. cross-workspace defense: a stale workspace_id makes every
        //    read fail with NotFound.
        let other_ctx = make_context_for_workspace(tmp.path(), "some-other-ws");
        let r = read.execute(json!({"id": note_id}), &other_ctx).await;
        assert!(!r.success, "cross-workspace read_note must be rejected");
        assert!(r.error.unwrap().contains("Not found"));
    }

    /// Verification gate 2: focused append behavior — three appends
    /// grow content linearly, and `updated_at` bumps on each call
    /// (so callers can detect "what changed since I last looked").
    #[tokio::test]
    async fn test_note_append() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, _bid, _cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        let create = CreateNoteTool { db: db.clone() };
        let r = create
            .execute(
                json!({"title": "log", "type": "general", "content": "v1"}),
                &ctx,
            )
            .await;
        assert!(r.success, "create_note: {:?}", r.error);
        let note_id = r.data["note"]["id"].as_str().unwrap().to_string();
        let initial_updated_at = r.data["note"]["updated_at"].as_str().unwrap().to_string();

        // rfc3339 has second precision; sleep past one second so the
        // bumps are observable.
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let append = AppendToNoteTool { db: db.clone() };
        let r1 = append
            .execute(json!({"id": note_id, "content": "-v2"}), &ctx)
            .await;
        assert!(r1.success, "append 1: {:?}", r1.error);
        assert_eq!(r1.data["note"]["content"], "v1-v2");
        let updated_at_1 = r1.data["note"]["updated_at"].as_str().unwrap().to_string();
        assert!(
            updated_at_1 > initial_updated_at,
            "updated_at must bump after first append ({} -> {})",
            initial_updated_at,
            updated_at_1
        );

        std::thread::sleep(std::time::Duration::from_millis(1100));

        let r2 = append
            .execute(json!({"id": note_id, "content": "-v3"}), &ctx)
            .await;
        assert!(r2.success, "append 2: {:?}", r2.error);
        assert_eq!(r2.data["note"]["content"], "v1-v2-v3");
        let updated_at_2 = r2.data["note"]["updated_at"].as_str().unwrap().to_string();
        assert!(updated_at_2 > updated_at_1, "updated_at must bump again");

        std::thread::sleep(std::time::Duration::from_millis(1100));

        let r3 = append
            .execute(json!({"id": note_id, "content": "-v4"}), &ctx)
            .await;
        assert!(r3.success, "append 3: {:?}", r3.error);
        assert_eq!(r3.data["note"]["content"], "v1-v2-v3-v4");
    }
}
