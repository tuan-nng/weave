//! `move_card` — move a task between columns with transition gates.
//!
//! Gate order:
//! 1. Resolve the task (workspace-scoped via `TaskStore::get_by_id`;
//!    cross-workspace access returns NotFound).
//! 2. Resolve the destination column. Same-board check
//!    (defense-in-depth — `TaskStore::move_to_column` also enforces).
//! 3. Run `service::kanban::check_transition_gates` on
//!    (task, source column, dest column) — rejects if the source
//!    freezes the description and the task's is blank, or if the dest
//!    requires fields/artifacts the task doesn't have.
//! 4. Delegate the actual move to `TaskStore::move_to_column` (which
//!    re-validates the same-board invariant and triggers a position
//!    rebalance in the destination column).
//!
//! Intentionally does NOT fire `try_automate_lane` or broadcast SSE
//! events. The HTTP PATCH path is the authoritative event source
//! until the runtime-dispatch feature lands.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::Db;
use crate::error::AppError;
use crate::service::kanban::check_transition_gates;
use crate::store::artifacts::ArtifactStore;
use crate::store::columns::{Column, ColumnStore};
use crate::store::tasks::{TaskStore, VALID_TASK_STATUSES};
use crate::tools::fs::{check_optional_status, error, optional_string, require_string, success};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

/// Validate that a column move does not skip more than one stage.
///
/// Same-column moves are not transitions and should be handled by the
/// caller before calling this function. The rule: the absolute
/// difference between source and dest stage indices must be ≤ 2
/// (allowing one skip, e.g., backlog→dev).
fn validate_stage_transition(source: &Column, dest: &Column) -> Result<(), AppError> {
    if source.id == dest.id {
        return Ok(());
    }
    let src_idx = source.stage.index();
    let dst_idx = dest.stage.index();
    let distance = (dst_idx - src_idx).abs();
    if distance > 2 {
        return Err(AppError::validation(format!(
            "cannot move from '{}' ({}) to '{}' ({}) — skips more than one stage; \
             move through intermediate stages first",
            source.name, source.stage, dest.name, dest.stage,
        )));
    }
    Ok(())
}

pub struct MoveCardTool {
    pub db: Arc<Db>,
}

#[async_trait]
impl ToolExecutor for MoveCardTool {
    fn name(&self) -> &str {
        "move_card"
    }

    fn description(&self) -> &str {
        "Move a card (task) to a different column on the same board. \
         Enforces transition gates: if the source column freezes \
         descriptions, the task must have a non-empty description; \
         if the destination column requires specific fields, they must \
         be set on the task; if the destination column requires specific \
         artifact types, they must be attached via the `provide_artifact` \
         tool first. Returns the moved card."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "card_id": {
                    "type": "string",
                    "description": "The card (task) ID to move."
                },
                "column_id": {
                    "type": "string",
                    "description": "The destination column ID. Must belong to the card's current board."
                },
                "position": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Optional 0-based position within the destination column. Omit to append at the end."
                },
                "status": {
                    "type": "string",
                    "description": format!(
                        "Optional. Update the card's status at the same time. Valid: {}",
                        VALID_TASK_STATUSES.join(", ")
                    )
                }
            },
            "required": ["card_id", "column_id"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let card_id = match require_string(&input, "card_id") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let dest_column_id = match require_string(&input, "column_id") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let position = input.get("position").and_then(|v| v.as_i64());
        let status = optional_string(&input, "status");
        if let Err(e) = check_optional_status(status.as_deref()) {
            return e;
        }

        // 1. Resolve task (workspace-scoped).
        let task = match TaskStore::get_by_id(&self.db, &card_id, &ctx.workspace_id) {
            Ok(t) => t,
            Err(e) => return error(e.to_string()),
        };

        // 2. Resolve source + destination columns.
        let source_column = match ColumnStore::get_by_id(&self.db, &task.column_id) {
            Ok(c) => c,
            Err(e) => return error(e.to_string()),
        };
        let dest_column = match ColumnStore::get_by_id(&self.db, &dest_column_id) {
            Ok(c) => c,
            Err(e) => return error(e.to_string()),
        };
        if dest_column.board_id != task.board_id {
            return error(format!(
                "column '{}' does not belong to board '{}'",
                dest_column_id, task.board_id
            ));
        }
        // Defensive: source column must be on the same board as the task.
        // Store invariants should make this unreachable, but a corrupt
        // row would otherwise let the gate evaluate a column from a
        // different board.
        if source_column.board_id != task.board_id {
            return error(format!(
                "internal: source column '{}' is not on task board '{}' \
                 (data invariant violated)",
                source_column.id, task.board_id
            ));
        }

        // 3. Stage transition check. Moves that skip more than one stage
        //    are rejected (e.g., backlog→review is forbidden; backlog→dev
        //    is allowed). Same-column moves short-circuit here.
        if let Err(e) = validate_stage_transition(&source_column, &dest_column) {
            return error(e.to_string());
        }

        // 4. Transition gates. `check_transition_gates` itself short-circuits
        //    on same-column moves and the all-defaults case, so this is
        //    a no-op for the common path. The artifact-type set is
        //    pre-loaded here (feat-031) so the gate stays a pure
        //    function over its inputs.
        let present_artifact_types =
            match ArtifactStore::list_types_for_task(&self.db, &task.id, &ctx.workspace_id) {
                Ok(set) => set,
                Err(e) => return error(e.to_string()),
            };
        if let Err(e) =
            check_transition_gates(&task, &source_column, &dest_column, &present_artifact_types)
        {
            return error(e.to_string());
        }

        // 4. Apply the move.
        let mut moved = match TaskStore::move_to_column(
            &self.db,
            &card_id,
            &ctx.workspace_id,
            &dest_column_id,
            position,
        ) {
            Ok(t) => t,
            Err(e) => return error(e.to_string()),
        };

        // 5. Optional status update on the same row. Skip the write when
        //    the requested status matches the current one (avoids a
        //    redundant UPDATE + `updated_at` bump).
        if let Some(ref s) = status {
            if s != &moved.status {
                moved = match TaskStore::update_status(&self.db, &card_id, &ctx.workspace_id, s) {
                    Ok(t) => t,
                    Err(e) => return error(e.to_string()),
                };
            }
        }

        success(json!({"task": moved}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::columns::{ColumnStage, ColumnStore};
    use crate::store::kanban_test_helpers::make_test_db;
    use crate::tools::test_support::make_context;
    use tempfile::TempDir;

    const TEST_WS: &str = "test-workspace";

    /// Seed: workspace → board → source col, dest col (no policy).
    /// Returns (board_id, source_col_id, dest_col_id, task_id).
    fn seed_basic(db: &Db) -> (String, String, String, String) {
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'ws', 'active', ?2, ?2)",
                rusqlite::params![TEST_WS, now],
            )
            .unwrap();
        let bid = uuid::Uuid::new_v4().to_string();
        let src_id = uuid::Uuid::new_v4().to_string();
        let dst_id = uuid::Uuid::new_v4().to_string();
        let task_id = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at)
                 VALUES (?1, ?2, 'board', ?3)",
                rusqlite::params![bid, TEST_WS, now],
            )
            .unwrap();
        for (cid, name, pos) in [(&src_id, "src", 0i64), (&dst_id, "dst", 1024)] {
            db.conn()
                .execute(
                    "INSERT INTO columns (id, board_id, name, position, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![cid, bid, name, pos, now],
                )
                .unwrap();
        }
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Card', 0, 'active', ?4, ?4)",
                rusqlite::params![task_id, bid, src_id, now],
            )
            .unwrap();
        (bid, src_id, dst_id, task_id)
    }

    /// Seed a board with a `freeze_description=true` source and an
    /// `required_fields=...` destination.
    fn seed_with_gates(
        db: &Db,
        freeze_source: bool,
        dest_required: Vec<String>,
    ) -> (String, String, String, String) {
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'ws', 'active', ?2, ?2)",
                rusqlite::params![TEST_WS, now],
            )
            .unwrap();
        let bid = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at)
                 VALUES (?1, ?2, 'board', ?3)",
                rusqlite::params![bid, TEST_WS, now],
            )
            .unwrap();
        // Source column (with optional freeze). Capture .id from the
        // returned Column so we don't re-query by name.
        let src_id = ColumnStore::create(
            db,
            &bid,
            "src",
            Some(0),
            None,
            false,
            Some(freeze_source),
            None,
            None,
            None,
            ColumnStage::Dev,
        )
        .unwrap()
        .id;
        // Dest column (with optional required fields).
        let dst_id = ColumnStore::create(
            db,
            &bid,
            "dst",
            Some(1024),
            None,
            false,
            None,
            if dest_required.is_empty() {
                None
            } else {
                Some(&dest_required)
            },
            None,
            None,
            ColumnStage::Dev,
        )
        .unwrap()
        .id;
        let task_id = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Card', 0, 'active', ?4, ?4)",
                rusqlite::params![task_id, bid, src_id, now],
            )
            .unwrap();
        (bid, src_id, dst_id, task_id)
    }

    #[tokio::test]
    async fn test_move_card_success() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (_bid, _src, dst, task_id) = seed_basic(&db);

        let tool = MoveCardTool { db };
        let result = tool
            .execute(json!({"card_id": task_id, "column_id": dst}), &ctx)
            .await;

        assert!(result.success, "got: {:?}", result.error);
        assert_eq!(result.data["task"]["column_id"], dst);
        assert_eq!(result.data["task"]["id"], task_id);
    }

    #[tokio::test]
    async fn test_move_card_missing_required_field() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (_bid, _src, dst, task_id) = seed_basic(&db);

        let tool = MoveCardTool { db };
        // Missing card_id
        let r1 = tool.execute(json!({"column_id": dst}), &ctx).await;
        assert!(!r1.success);
        assert!(r1.error.unwrap().contains("Missing"));
        // Missing column_id
        let r2 = tool.execute(json!({"card_id": task_id}), &ctx).await;
        assert!(!r2.success);
        assert!(r2.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_move_card_task_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (_bid, _src, dst, _task_id) = seed_basic(&db);

        let tool = MoveCardTool { db };
        let result = tool
            .execute(json!({"card_id": "nonexistent", "column_id": dst}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_move_card_column_not_found() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (_bid, _src, _dst, task_id) = seed_basic(&db);

        let tool = MoveCardTool { db };
        let result = tool
            .execute(
                json!({"card_id": task_id, "column_id": "nonexistent"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_move_card_cross_workspace() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path()); // workspace_id = "test-workspace"
        let db = make_test_db();
        let now = chrono::Utc::now().to_rfc3339();
        let other_ws = "other-workspace";
        let other_bid = uuid::Uuid::new_v4().to_string();
        let other_cid = uuid::Uuid::new_v4().to_string();
        let other_task = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'other', 'active', ?2, ?2)",
                rusqlite::params![other_ws, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at)
                 VALUES (?1, ?2, 'b', ?3)",
                rusqlite::params![other_bid, other_ws, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO columns (id, board_id, name, position, created_at)
                 VALUES (?1, ?2, 'c', 0, ?3)",
                rusqlite::params![other_cid, other_bid, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 't', 0, 'active', ?4, ?4)",
                rusqlite::params![other_task, other_bid, other_cid, now],
            )
            .unwrap();

        let tool = MoveCardTool { db };
        let result = tool
            .execute(json!({"card_id": other_task, "column_id": other_cid}), &ctx)
            .await;

        assert!(!result.success, "cross-workspace access must be rejected");
        assert!(result.error.unwrap().contains("Not found"));
    }

    #[tokio::test]
    async fn test_move_card_column_wrong_board() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (_bid, _src, _dst, task_id) = seed_basic(&db);
        // Add a second board + column in the same workspace.
        let now = chrono::Utc::now().to_rfc3339();
        let other_bid = uuid::Uuid::new_v4().to_string();
        let other_cid = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at)
                 VALUES (?1, ?2, 'b2', ?3)",
                rusqlite::params![other_bid, TEST_WS, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO columns (id, board_id, name, position, created_at)
                 VALUES (?1, ?2, 'c', 0, ?3)",
                rusqlite::params![other_cid, other_bid, now],
            )
            .unwrap();

        let tool = MoveCardTool { db };
        let result = tool
            .execute(json!({"card_id": task_id, "column_id": other_cid}), &ctx)
            .await;

        assert!(!result.success, "cross-board move must be rejected");
        assert!(result.error.unwrap().contains("does not belong"));
    }

    // -----------------------------------------------------------------------
    // Transition-gate integration tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_move_card_freeze_blocks_empty_description() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (_bid, _src, dst, task_id) = seed_with_gates(&db, true, vec![]);

        let tool = MoveCardTool { db };
        let result = tool
            .execute(json!({"card_id": task_id, "column_id": dst}), &ctx)
            .await;

        assert!(!result.success, "freeze on empty description must fail");
        let err = result.error.unwrap();
        assert!(err.contains("freezes descriptions"), "got: {err}");
    }

    #[tokio::test]
    async fn test_move_card_freeze_passes_when_description_set() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (_bid, _src, dst, task_id) = seed_with_gates(&db, true, vec![]);
        // Set the task's description.
        db.conn()
            .execute(
                "UPDATE tasks SET description = 'body' WHERE id = ?1",
                [task_id.as_str()],
            )
            .unwrap();

        let tool = MoveCardTool { db };
        let result = tool
            .execute(json!({"card_id": task_id, "column_id": dst}), &ctx)
            .await;

        assert!(result.success, "got: {:?}", result.error);
        assert_eq!(result.data["task"]["column_id"], dst);
    }

    #[tokio::test]
    async fn test_move_card_required_field_blocks_missing() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        // Pick a known required field; the task starts with all fields None.
        let first_field = "acceptance_criteria".to_string();
        let (_bid, _src, dst, task_id) = seed_with_gates(&db, false, vec![first_field.clone()]);

        let tool = MoveCardTool { db };
        let result = tool
            .execute(json!({"card_id": task_id, "column_id": dst}), &ctx)
            .await;

        assert!(!result.success, "missing required field must fail");
        let err = result.error.unwrap();
        assert!(err.contains(&first_field), "got: {err}");
        assert!(err.contains("non-empty"), "got: {err}");
    }

    #[tokio::test]
    async fn test_move_card_required_field_passes_when_set() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let first_field = "acceptance_criteria".to_string();
        let (_bid, _src, dst, task_id) = seed_with_gates(&db, false, vec![first_field.clone()]);
        // Set the required field on the task via raw SQL. The
        // `first_field` value is hard-coded to one of the three valid
        // required-field column names (acceptance_criteria /
        // completion_summary / verification_report), so the
        // interpolation is safe — but the pattern is a footgun; if
        // this test ever takes a user-controlled value, switch to a
        // `match` over a fixed set of column names.
        db.conn()
            .execute(
                &format!("UPDATE tasks SET {first_field} = 'set' WHERE id = ?1"),
                [task_id.as_str()],
            )
            .unwrap();

        let tool = MoveCardTool { db };
        let result = tool
            .execute(json!({"card_id": task_id, "column_id": dst}), &ctx)
            .await;

        assert!(result.success, "got: {:?}", result.error);
        assert_eq!(result.data["task"]["column_id"], dst);
    }

    #[tokio::test]
    async fn test_move_card_with_explicit_position() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (_bid, _src, dst, task_id) = seed_basic(&db);
        // Seed a second task already in `dst` so position=0 is meaningful.
        let now = chrono::Utc::now().to_rfc3339();
        let existing = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Existing', 0, 'active', ?4, ?4)",
                rusqlite::params![existing, _bid, dst, now],
            )
            .unwrap();

        let tool = MoveCardTool { db };
        let result = tool
            .execute(
                json!({"card_id": task_id, "column_id": dst, "position": 0}),
                &ctx,
            )
            .await;

        assert!(result.success, "got: {:?}", result.error);
        assert_eq!(result.data["task"]["column_id"], dst);
        assert_eq!(result.data["task"]["position"], 0);
    }

    #[tokio::test]
    async fn test_move_card_with_status_update() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (_bid, _src, dst, task_id) = seed_basic(&db);

        let tool = MoveCardTool { db };
        let result = tool
            .execute(
                json!({"card_id": task_id, "column_id": dst, "status": "done"}),
                &ctx,
            )
            .await;

        assert!(result.success, "got: {:?}", result.error);
        assert_eq!(result.data["task"]["status"], "done");
        assert_eq!(result.data["task"]["column_id"], dst);
    }

    #[tokio::test]
    async fn test_move_card_with_invalid_status() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (_bid, _src, dst, task_id) = seed_basic(&db);

        let tool = MoveCardTool { db };
        let result = tool
            .execute(
                json!({"card_id": task_id, "column_id": dst, "status": "bogus"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("invalid task status"));
    }

    #[tokio::test]
    async fn test_move_card_same_column_noop() {
        // Same-column move short-circuits via `check_transition_gates`,
        // but `TaskStore::move_to_column` will still rewrite the
        // position. The tool's contract: it succeeds, returns the
        // task, and the column_id is unchanged.
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let db = make_test_db();
        let (_bid, src, _dst, task_id) = seed_basic(&db);

        let tool = MoveCardTool { db };
        let result = tool
            .execute(json!({"card_id": task_id, "column_id": src}), &ctx)
            .await;

        assert!(result.success, "got: {:?}", result.error);
        assert_eq!(result.data["task"]["column_id"], src);
    }
}
