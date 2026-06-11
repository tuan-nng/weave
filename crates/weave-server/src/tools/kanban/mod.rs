//! Kanban tools for agents (feat-028).
//!
//! Four tool executors that give an agent autonomous access to the kanban
//! surface:
//!
//! - `get_board` — return a composite board (board + columns + tasks).
//! - `create_card` — insert a new task on a column.
//! - `search_cards` — list tasks with optional filters (board, column,
//!   status) and free-text query against title + description.
//! - `move_card` — move a task between columns, with transition gates
//!   enforced on the source and destination columns (see
//!   `service::kanban::check_transition_gates`).
//!
//! All four tools hold `Arc<Db>` (consistent with the other DB-backed
//! tools in `tools/task/`). `MoveCardTool` is a thin wrapper: it
//! resolves the task + columns, runs the gate check, then delegates to
//! `TaskStore::move_to_column`. It does NOT fire `try_automate_lane`
//! or broadcast SSE events — the HTTP PATCH path remains the
//! authoritative event source until the runtime-dispatch feature
//! lands.

pub mod create_card;
pub mod get_board;
pub mod move_card;
pub mod search_cards;

pub use create_card::CreateCardTool;
pub use get_board::GetBoardTool;
pub use move_card::MoveCardTool;
pub use search_cards::SearchCardsTool;

#[cfg(test)]
mod tests {
    //! Verification-gate tests for feat-028.
    //!
    //! The two tests in this module are the named verification targets
    //! from `feature_list.json`:
    //!
    //! - `test_kanban_tools` — exercises all four tool executors in a
    //!   single fixture: `get_board`, `create_card`, `search_cards`,
    //!   `move_card`. This proves the tools are registered, callable,
    //!   and return the expected shapes.
    //! - `test_move_card_transition_gates` — exercises each transition
    //!   gate (description frozen on exit, required fields on entry) as
    //!   well as the "all gates pass" happy path. This is the heart of
    //!   the spec line `move_card (enforces transition gates — required
    //!   artifacts, required fields, description frozen from dev stage)`.
    use super::*;
    use crate::store::columns::ColumnStore;
    use crate::store::kanban_test_helpers::{make_test_db, seed_workspace_with_board};
    use crate::tools::test_support::make_context_for_workspace;
    use crate::tools::ToolExecutor;
    use serde_json::json;
    use tempfile::TempDir;

    /// Verification gate 1: exercise all four kanban tools in one fixture.
    #[tokio::test]
    async fn test_kanban_tools() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        // 1. get_board — board exists, returns composite.
        let get_board = GetBoardTool { db: db.clone() };
        let r = get_board.execute(json!({"board_id": bid}), &ctx).await;
        assert!(r.success, "get_board: {:?}", r.error);
        assert_eq!(r.data["board"]["id"], bid);
        assert_eq!(r.data["columns"].as_array().unwrap().len(), 1);
        assert_eq!(r.data["tasks"].as_array().unwrap().len(), 0);

        // 2. create_card — insert a new card on the column.
        let create = CreateCardTool { db: db.clone() };
        let r = create
            .execute(
                json!({
                    "board_id": bid,
                    "column_id": cid,
                    "title": "Implement kanban tools",
                    "description": "Wire up get_board, create_card, search_cards, move_card."
                }),
                &ctx,
            )
            .await;
        assert!(r.success, "create_card: {:?}", r.error);
        let new_card_id = r.data["task"]["id"].as_str().unwrap().to_string();

        // 3. search_cards — query by free-text fragment matches the new card.
        let search = SearchCardsTool { db: db.clone() };
        let r = search.execute(json!({"query": "kanban tools"}), &ctx).await;
        assert!(r.success, "search_cards(query): {:?}", r.error);
        assert_eq!(r.data["count"], 1);
        assert_eq!(r.data["tasks"][0]["id"], new_card_id);

        // 4. search_cards — no filter returns all (1 card now).
        let r = search.execute(json!({}), &ctx).await;
        assert!(r.success);
        assert_eq!(r.data["count"], 1);

        // 5. search_cards — combined query + board_id.
        let r = search
            .execute(json!({"query": "kanban", "board_id": bid}), &ctx)
            .await;
        assert!(r.success);
        assert_eq!(r.data["count"], 1);

        // 6. search_cards — status filter that doesn't match returns 0.
        let r = search.execute(json!({"status": "done"}), &ctx).await;
        assert!(r.success);
        assert_eq!(r.data["count"], 0);

        // 7. move_card — move the new card to a no-policy destination.
        //    Use the store API so the test exercises the same code path
        //    real callers do (defaults for freeze/required fields are
        //    set by ColumnStore::create, not by SQL column defaults).
        let dest_cid = ColumnStore::create(
            &db,
            &bid,
            "done",
            Some(2048),
            None,
            false,
            None,
            None,
            None,
            None,
        )
        .unwrap()
        .id;
        let move_card = MoveCardTool { db: db.clone() };
        let r = move_card
            .execute(json!({"card_id": new_card_id, "column_id": dest_cid}), &ctx)
            .await;
        assert!(r.success, "move_card: {:?}", r.error);
        assert_eq!(r.data["task"]["column_id"], dest_cid);

        // 8. cross-workspace defense: a stale workspace_id in ctx
        //    makes every read fail with NotFound. Use a fresh context
        //    with a workspace the seeded board does NOT belong to.
        let other_ctx = make_context_for_workspace(tmp.path(), "some-other-ws");
        let r = get_board
            .execute(json!({"board_id": bid}), &other_ctx)
            .await;
        assert!(!r.success, "cross-workspace get_board must be rejected");
        assert!(r.error.unwrap().contains("Not found"));
    }

    /// Verification gate 2: exercise the three transition gates.
    ///
    /// The test runs a single card through 4 columns with different gate
    /// configurations and asserts the right gate fires (or doesn't).
    #[tokio::test]
    async fn test_move_card_transition_gates() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, _src_cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        // Build a board with 4 columns: src (no policy), frozen (freeze desc),
        // gated (requires acceptance_criteria + verification_report),
        // open (no policy). Insert one bare card on `src`.
        let now = chrono::Utc::now().to_rfc3339();

        // Rename the seed column to "src" for clarity.
        let src_cid: String = db
            .conn()
            .query_row(
                "SELECT id FROM columns WHERE board_id = ?1 LIMIT 1",
                [bid.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        db.conn()
            .execute(
                "UPDATE columns SET name = 'src', position = 0 WHERE id = ?1",
                [src_cid.as_str()],
            )
            .unwrap();

        // Add the 3 other columns via the public store API.
        let frozen_cid = ColumnStore::create(
            &db,
            &bid,
            "frozen",
            Some(1024),
            None,
            false,
            Some(true), // freeze_description = true
            None,
            None,
            None,
        )
        .unwrap()
        .id;
        let gated_cid = ColumnStore::create(
            &db,
            &bid,
            "gated",
            Some(2048),
            None,
            false,
            None,
            Some(&[
                "acceptance_criteria".to_string(),
                "verification_report".to_string(),
            ]),
            None,
            None,
        )
        .unwrap()
        .id;
        let _open_cid = ColumnStore::create(
            &db,
            &bid,
            "open",
            Some(3072),
            None,
            false,
            None,
            None,
            None,
            None,
        )
        .unwrap()
        .id;

        // Create a bare card on `src` — no description, no AC, no VR.
        let card_id = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Card', 0, 'active', ?4, ?4)",
                rusqlite::params![card_id, bid, src_cid, now],
            )
            .unwrap();

        let move_card = MoveCardTool { db: db.clone() };

        // 1. src → frozen: SUCCEEDS (src has no freeze; freeze is enforced
        //    on EXIT, not entry. The card is now in `frozen`.)
        let r = move_card
            .execute(json!({"card_id": card_id, "column_id": frozen_cid}), &ctx)
            .await;
        assert!(r.success, "src → frozen (entry to freeze): {:?}", r.error);
        assert_eq!(r.data["task"]["column_id"], frozen_cid);

        // 2. frozen → gated (with blank desc): REJECTED — frozen freezes
        //    descriptions on exit, and the task has no description.
        let r = move_card
            .execute(json!({"card_id": card_id, "column_id": gated_cid}), &ctx)
            .await;
        assert!(
            !r.success,
            "frozen → gated with blank desc must fail (freeze on exit)"
        );
        let err = r.error.unwrap();
        assert!(err.contains("freezes descriptions"), "got: {err}");

        // 3. frozen → open: REJECTED for the same reason (blank desc on exit).
        let r = move_card
            .execute(json!({"card_id": card_id, "column_id": _open_cid}), &ctx)
            .await;
        assert!(
            !r.success,
            "frozen → open with blank desc must fail (freeze on exit)"
        );

        // 4. frozen → frozen (same-column no-op): SUCCEEDS.
        let r = move_card
            .execute(json!({"card_id": card_id, "column_id": frozen_cid}), &ctx)
            .await;
        assert!(r.success, "frozen → frozen no-op: {:?}", r.error);

        // 5. Set description on the task. Now frozen → gated should
        //    reach the required-fields check, which also fails (no AC/VR).
        db.conn()
            .execute(
                "UPDATE tasks SET description = 'body' WHERE id = ?1",
                [card_id.as_str()],
            )
            .unwrap();
        let r = move_card
            .execute(json!({"card_id": card_id, "column_id": gated_cid}), &ctx)
            .await;
        assert!(
            !r.success,
            "frozen → gated with desc but no AC must fail (required fields)"
        );
        let err = r.error.unwrap();
        assert!(err.contains("acceptance_criteria"), "got: {err}");

        // 6. frozen → src (with desc, no required fields on src): SUCCEEDS.
        //    Returns the card to the no-policy column.
        let r = move_card
            .execute(json!({"card_id": card_id, "column_id": src_cid}), &ctx)
            .await;
        assert!(r.success, "frozen → src with desc: {:?}", r.error);
        assert_eq!(r.data["task"]["column_id"], src_cid);

        // 7. src → gated: REJECTED (no AC/VR; src has no freeze, so the
        //    failure is from the required-fields gate, not the freeze gate).
        let r = move_card
            .execute(json!({"card_id": card_id, "column_id": gated_cid}), &ctx)
            .await;
        assert!(
            !r.success,
            "src → gated without AC/VR must fail (required fields)"
        );
        let err = r.error.unwrap();
        assert!(err.contains("acceptance_criteria"), "got: {err}");

        // 8. Set AC + VR. Now src → gated SUCCEEDS (all gates pass).
        db.conn()
            .execute(
                "UPDATE tasks SET acceptance_criteria = 'AC',
                                  verification_report = 'VR'
                 WHERE id = ?1",
                [card_id.as_str()],
            )
            .unwrap();
        let r = move_card
            .execute(json!({"card_id": card_id, "column_id": gated_cid}), &ctx)
            .await;
        assert!(
            r.success,
            "src → gated with all fields set must succeed: {:?}",
            r.error
        );
        assert_eq!(r.data["task"]["column_id"], gated_cid);
    }
}
