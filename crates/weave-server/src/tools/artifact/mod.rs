//! Artifact tools for agents (feat-031).
//!
//! Three tool executors that give an agent autonomous access to the
//! artifact surface (evidence attached to a kanban task):
//!
//! - `request_artifact` — declare intent to attach an artifact by
//!   `(task_id, type)`. Creates a new row with empty content. If a
//!   row already exists for that pair, returns Conflict (use
//!   `provide_artifact` to amend).
//! - `provide_artifact` — upsert artifact content by `(task_id, type)`.
//!   Updates `content` + `updated_at` if a row exists, inserts otherwise.
//!   Returns the row id either way.
//! - `list_artifacts` — workspace-scoped list of artifacts attached
//!   to a task, with an optional `type` filter.
//!
//! All three tools hold `Arc<Db>` (consistent with the other DB-backed
//! tools in `tools/task/` and `tools/kanban/`). They deliberately do
//! NOT call `check_transition_gates` or fire SSE events — the agent
//! invokes them at any time, independent of the move path. The move
//! path (the `move_card` tool) is the authoritative gate enforcer.

pub mod list_artifacts;
pub mod provide_artifact;
pub mod request_artifact;

pub use list_artifacts::ListArtifactsTool;
pub use provide_artifact::ProvideArtifactTool;
pub use request_artifact::RequestArtifactTool;

#[cfg(test)]
mod tests {
    //! Verification-gate tests for feat-031.
    //!
    //! The two tests in this module are the named verification targets
    //! from `feature_list.json`:
    //!
    //! - `test_artifact_crud` — exercises all three tool executors in a
    //!   single fixture: `request_artifact`, `provide_artifact`,
    //!   `list_artifacts`. Proves the tools are registered, callable,
    //!   and return the expected shapes.
    //! - `test_artifact_transition_gate` — exercises the gate end-to-end
    //!   through `MoveCardTool`: a column with `required_artifact_types`
    //!   blocks a move when the task has no artifacts, blocks when the
    //!   task has a different type, succeeds once `provide_artifact`
    //!   adds the required type, and re-blocks when the artifact is
    //!   moved to a column with a different gate.
    use super::*;
    use crate::store::columns::{ColumnStage, ColumnStore};
    use crate::store::kanban_test_helpers::{make_test_db, seed_workspace_with_board};
    use crate::tools::kanban::MoveCardTool;
    use crate::tools::test_support::make_context_for_workspace;
    use crate::tools::ToolExecutor;
    use serde_json::json;
    use tempfile::TempDir;

    /// Verification gate 1: exercise all three artifact tools in one fixture.
    #[tokio::test]
    async fn test_artifact_crud() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        // Seed a task on the column.
        let task_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Card', 0, 'active', ?4, ?4)",
                rusqlite::params![task_id, bid, cid, now],
            )
            .unwrap();

        // 1. request_artifact — creates a fresh row with empty content.
        let req = RequestArtifactTool { db: db.clone() };
        let r = req
            .execute(
                json!({"task_id": task_id, "type": "screenshot", "content": ""}),
                &ctx,
            )
            .await;
        assert!(r.success, "request_artifact: {:?}", r.error);
        let req_id = r.data["artifact"]["id"].as_str().unwrap().to_string();
        assert_eq!(r.data["artifact"]["content"], "");
        assert_eq!(r.data["artifact"]["type"], "screenshot");

        // 2. list_artifacts — sees the new row.
        let list = ListArtifactsTool { db: db.clone() };
        let r = list.execute(json!({"task_id": task_id}), &ctx).await;
        assert!(r.success, "list_artifacts: {:?}", r.error);
        assert_eq!(r.data["count"], 1);
        assert_eq!(r.data["artifacts"][0]["id"], req_id);

        // 3. provide_artifact — upserts content on the existing row.
        let prov = ProvideArtifactTool { db: db.clone() };
        let r = prov
            .execute(
                json!({
                    "task_id": task_id,
                    "type": "screenshot",
                    "content": "img-bytes-v2"
                }),
                &ctx,
            )
            .await;
        assert!(r.success, "provide_artifact: {:?}", r.error);
        let upd_id = r.data["artifact"]["id"].as_str().unwrap().to_string();
        // id is preserved (upsert, not create).
        assert_eq!(upd_id, req_id, "upsert preserves id");
        assert_eq!(r.data["artifact"]["content"], "img-bytes-v2");

        // 4. provide_artifact on a different type — new row.
        let r = prov
            .execute(
                json!({
                    "task_id": task_id,
                    "type": "log",
                    "content": "trace output"
                }),
                &ctx,
            )
            .await;
        assert!(r.success, "provide_artifact (new type): {:?}", r.error);
        let log_id = r.data["artifact"]["id"].as_str().unwrap().to_string();
        assert_ne!(log_id, req_id);

        // 5. list_artifacts with type filter — narrows to one.
        let r = list
            .execute(json!({"task_id": task_id, "type": "log"}), &ctx)
            .await;
        assert!(r.success);
        assert_eq!(r.data["count"], 1);
        assert_eq!(r.data["artifacts"][0]["type"], "log");

        // 6. list_artifacts without filter — sees both.
        let r = list.execute(json!({"task_id": task_id}), &ctx).await;
        assert!(r.success);
        assert_eq!(r.data["count"], 2);

        // 7. cross-workspace defense: a stale workspace_id makes every
        //    read fail with NotFound.
        let other_ctx = make_context_for_workspace(tmp.path(), "some-other-ws");
        let r = list.execute(json!({"task_id": task_id}), &other_ctx).await;
        assert!(
            !r.success,
            "cross-workspace list_artifacts must be rejected"
        );
        assert!(r.error.unwrap().contains("Not found"));
    }

    /// Verification gate 2: exercise the artifact gate end-to-end
    /// through `MoveCardTool`. The four-step progression proves:
    ///
    ///   1. missing artifacts block the move;
    ///   2. wrong-type artifacts still block (the gate compares
    ///      `task`'s present types against `column.required_artifact_types`);
    ///   3. providing the right type unblocks the move;
    ///   4. moving to a column with a *different* `required_artifact_types`
    ///      re-blocks the move, proving the gate is evaluated on every
    ///      move (not cached against the task).
    #[tokio::test]
    async fn test_artifact_transition_gate() {
        let tmp = TempDir::new().unwrap();
        let db = make_test_db();
        let (ws, bid, src_cid) = seed_workspace_with_board(&db);
        let ctx = make_context_for_workspace(tmp.path(), &ws);

        // Destination column A requires `["screenshot"]`.
        let dst_a = ColumnStore::create(
            &db,
            &bid,
            "review",
            Some(1024),
            None,
            false,
            None,
            None,
            Some(&["screenshot".to_string()]),
            None,
            ColumnStage::Dev,
            None,
        )
        .unwrap()
        .id;
        // Destination column B requires `["test_results"]` — a
        // *different* artifact, so once the card is in column A with
        // only a `screenshot` artifact, moving to B must re-block.
        let dst_b = ColumnStore::create(
            &db,
            &bid,
            "qa",
            Some(1024),
            None,
            false,
            None,
            None,
            Some(&["test_results".to_string()]),
            None,
            ColumnStage::Dev,
            None,
        )
        .unwrap()
        .id;

        // Seed a bare task with no artifacts.
        let task_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Card', 0, 'active', ?4, ?4)",
                rusqlite::params![task_id, bid, src_cid, now],
            )
            .unwrap();

        let move_card = MoveCardTool { db: db.clone() };
        let prov = ProvideArtifactTool { db: db.clone() };

        // 1. No artifacts → gate blocks.
        let r = move_card
            .execute(json!({"card_id": task_id, "column_id": dst_a}), &ctx)
            .await;
        assert!(!r.success, "missing artifact must fail the gate");
        let err = r.error.unwrap();
        assert!(err.contains("screenshot"), "got: {err}");
        assert!(err.contains("provide_artifact"), "got: {err}");

        // 2. Wrong type → gate still blocks.
        let prep = prov
            .execute(
                json!({"task_id": task_id, "type": "log", "content": "x"}),
                &ctx,
            )
            .await;
        assert!(prep.success, "seed log artifact: {:?}", prep.error);
        let r = move_card
            .execute(json!({"card_id": task_id, "column_id": dst_a}), &ctx)
            .await;
        assert!(!r.success, "wrong-type artifact must not satisfy the gate");
        assert!(r.error.unwrap().contains("screenshot"));

        // 3. Right type via provide_artifact → gate passes.
        let prep = prov
            .execute(
                json!({"task_id": task_id, "type": "screenshot", "content": "img"}),
                &ctx,
            )
            .await;
        assert!(prep.success, "seed screenshot artifact: {:?}", prep.error);
        let r = move_card
            .execute(json!({"card_id": task_id, "column_id": dst_a}), &ctx)
            .await;
        assert!(
            r.success,
            "right-type artifact must satisfy the gate: {:?}",
            r.error
        );
        assert_eq!(r.data["task"]["column_id"], dst_a);

        // 4. Re-block: a *different* destination column with a
        //    different required_artifact_types must still gate the
        //    move, even though the card already has a `screenshot`
        //    artifact. Proves the gate is re-evaluated on every
        //    transition, not carried over from the previous one.
        let r = move_card
            .execute(json!({"card_id": task_id, "column_id": dst_b}), &ctx)
            .await;
        assert!(
            !r.success,
            "moving to a column requiring a different artifact must re-block"
        );
        let err = r.error.unwrap();
        assert!(err.contains("test_results"), "got: {err}");
        assert!(err.contains("provide_artifact"), "got: {err}");
    }
}
