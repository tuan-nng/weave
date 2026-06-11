//! `columns` store — CRUD for kanban columns.
//!
//! Columns belong to a board. Each column has a position within its board
//! (sparse integers with rebalance), an optional `specialist_id` binding,
//! and an `auto_trigger` boolean.
//!
//! Validation invariant: `auto_trigger=true` requires `specialist_id`.
//! Enforced in `create` and `update` at the store layer so any caller
//! (HTTP handler, future service layer, agent tool) is safe.
//!
//! The `rebalance_column` free function renumbers all task positions
//! within a column to `i64 * 1024` spacing when the column's positions
//! have drifted too close together.
//!
//! Transition gate policy (added in feat-028, see migration 006):
//!   - `freeze_description`: when true, tasks in this column must have a
//!     non-empty `description` before they can be moved out. Enforced
//!     in `service::kanban::check_transition_gates`.
//!   - `required_fields`: JSON array of `tasks` field names that must be
//!     non-empty on tasks entering this column. Valid names:
//!     `acceptance_criteria`, `completion_summary`, `verification_report`.
//!   - `required_artifact_types`: JSON array of artifact type names that
//!     must be present on tasks entering this column. Schema-only for
//!     feat-028 (the real artifacts table is feat-031).

use rusqlite::Connection;
use serde::Serialize;
use uuid::Uuid;

use crate::db::Db;
use crate::error::AppError;
use chrono::Utc;

/// Domain representation of a column row.
#[derive(Debug, Clone, Serialize)]
pub struct Column {
    pub id: String,
    pub board_id: String,
    pub name: String,
    pub position: i64,
    pub specialist_id: Option<String>,
    pub auto_trigger: bool,
    /// When true, a task in this column must have a non-empty `description`
    /// before it can be moved out. Enforced in the move path
    /// (`service::kanban::check_transition_gates`).
    pub freeze_description: bool,
    /// JSON-decoded list of `tasks` field names that must be non-empty on
    /// tasks entering this column. Empty by default (no requirement).
    pub required_fields: Vec<String>,
    /// JSON-decoded list of artifact type names that must be present on
    /// tasks entering this column. Empty by default. Schema-only for
    /// feat-028 — the artifacts table (feat-031) is the source of truth.
    pub required_artifact_types: Vec<String>,
    /// Nullable Runtime Tool binding for lane automation. When set,
    /// `try_automate_lane` uses this to select a matching provider and
    /// set the session's `runtime_kind`. NULL means "inherit from the
    /// workspace's first provider" (the pre-feat-055 default).
    pub runtime_kind: Option<String>,
    pub created_at: String,
}

/// Minimum gap between adjacent positions. When `(next - prev) < MIN_GAP`,
/// `rebalance_column` renumbers the column.
pub const MIN_GAP: i64 = 2;

/// Default step between positions when inserting at the end (no `next`).
pub const POSITION_STEP: i64 = 1_000;

/// Stateless store for column persistence.
pub struct ColumnStore;

impl ColumnStore {
    /// Insert a new column. Validates the auto-trigger guard.
    ///
    /// Returns the created row, including its server-assigned `position`
    /// when `position` was `None` (uses `max(position) + POSITION_STEP`).
    ///
    /// The three transition-gate fields default to `false`/`[]`/`[]` when
    /// `None` is passed. Existing callers (HTTP API, default board
    /// template) can ignore them.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        db: &Db,
        board_id: &str,
        name: &str,
        position: Option<i64>,
        specialist_id: Option<&str>,
        auto_trigger: bool,
        freeze_description: Option<bool>,
        required_fields: Option<&[String]>,
        required_artifact_types: Option<&[String]>,
        runtime_kind: Option<&str>,
    ) -> Result<Column, AppError> {
        validate_auto_trigger(auto_trigger, specialist_id)?;
        db.with_transaction(|conn| {
            Self::create_tx(
                conn,
                board_id,
                name,
                position,
                specialist_id,
                auto_trigger,
                freeze_description,
                required_fields,
                required_artifact_types,
                runtime_kind,
            )
        })
    }

    /// Insert a column inside an existing transaction.
    ///
    /// Used by `BoardStore::create` to atomically insert template columns.
    /// Auto-trigger guard is also enforced here. The three transition-gate
    /// fields default to `false`/`[]`/`[]` when `None` is passed.
    #[allow(clippy::too_many_arguments)]
    pub fn create_tx(
        conn: &Connection,
        board_id: &str,
        name: &str,
        position: Option<i64>,
        specialist_id: Option<&str>,
        auto_trigger: bool,
        freeze_description: Option<bool>,
        required_fields: Option<&[String]>,
        required_artifact_types: Option<&[String]>,
        runtime_kind: Option<&str>,
    ) -> Result<Column, AppError> {
        validate_auto_trigger(auto_trigger, specialist_id)?;
        let resolved_position = match position {
            Some(p) => p,
            None => next_position_in_column(conn, board_id)?,
        };
        let freeze = freeze_description.unwrap_or(false);
        let req_fields_json = json_string_vec(required_fields.unwrap_or(&[]))?;
        let req_artifacts_json = json_string_vec(required_artifact_types.unwrap_or(&[]))?;
        conn.query_row(
            "INSERT INTO columns
                (id, board_id, name, position, specialist_id, auto_trigger,
                 freeze_description, required_fields, required_artifact_types,
                 runtime_kind, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             RETURNING id, board_id, name, position, specialist_id, auto_trigger,
                       freeze_description, required_fields, required_artifact_types,
                       runtime_kind, created_at",
            rusqlite::params![
                Uuid::new_v4().to_string(),
                board_id,
                name,
                resolved_position,
                specialist_id,
                auto_trigger as i64,
                freeze as i64,
                req_fields_json,
                req_artifacts_json,
                runtime_kind,
                Utc::now().to_rfc3339(),
            ],
            Self::map_row,
        )
        .map_err(AppError::from)
    }

    /// Update a column's editable fields.
    ///
    /// The three transition-gate fields are `Option`-wrapped: `None` means
    /// "no change", `Some(_)` means "set to this value". The
    /// `required_fields` / `required_artifact_types` lists replace the
    /// stored list wholesale (no merge).
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        db: &Db,
        column_id: &str,
        name: Option<&str>,
        position: Option<i64>,
        specialist_id: Option<Option<&str>>,
        auto_trigger: Option<bool>,
        freeze_description: Option<bool>,
        required_fields: Option<&[String]>,
        required_artifact_types: Option<&[String]>,
        runtime_kind: Option<Option<&str>>,
    ) -> Result<Column, AppError> {
        // Resolve the effective auto_trigger/specialist_id for validation.
        // The `Option<Option<T>>` pattern lets the caller pass:
        //   None       -> field not being changed
        //   Some(None) -> set to NULL
        //   Some(Some(v)) -> set to v
        let current = Self::get_by_id(db, column_id)?;
        let effective_auto_trigger = auto_trigger.unwrap_or(current.auto_trigger);
        let effective_specialist_id: Option<String> = match specialist_id {
            Some(s) => s.map(|x| x.to_string()),
            None => current.specialist_id.clone(),
        };
        validate_auto_trigger(effective_auto_trigger, effective_specialist_id.as_deref())?;

        let mut sets = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1u32;

        if let Some(n) = name {
            sets.push(format!("name = ?{idx}"));
            params.push(Box::new(n.to_string()));
            idx += 1;
        }
        if let Some(p) = position {
            sets.push(format!("position = ?{idx}"));
            params.push(Box::new(p));
            idx += 1;
        }
        if let Some(s) = specialist_id {
            sets.push(format!("specialist_id = ?{idx}"));
            params.push(Box::new(s.map(|x| x.to_string())));
            idx += 1;
        }
        if let Some(at) = auto_trigger {
            sets.push(format!("auto_trigger = ?{idx}"));
            params.push(Box::new(at as i64));
            idx += 1;
        }
        if let Some(f) = freeze_description {
            sets.push(format!("freeze_description = ?{idx}"));
            params.push(Box::new(f as i64));
            idx += 1;
        }
        if let Some(rf) = required_fields {
            let json = json_string_vec(rf)?;
            sets.push(format!("required_fields = ?{idx}"));
            params.push(Box::new(json));
            idx += 1;
        }
        if let Some(ra) = required_artifact_types {
            let json = json_string_vec(ra)?;
            sets.push(format!("required_artifact_types = ?{idx}"));
            params.push(Box::new(json));
            idx += 1;
        }
        if let Some(rk) = runtime_kind {
            sets.push(format!("runtime_kind = ?{idx}"));
            params.push(Box::new(rk.map(|x| x.to_string())));
            idx += 1;
        }

        // No-op when nothing to update — return current state.
        if sets.is_empty() {
            return Ok(current);
        }

        let cid_idx = idx;
        let sql = format!(
            "UPDATE columns SET {} WHERE id = ?{cid_idx}
             RETURNING id, board_id, name, position, specialist_id, auto_trigger,
                       freeze_description, required_fields, required_artifact_types,
                       runtime_kind, created_at",
            sets.join(", ")
        );
        params.push(Box::new(column_id.to_string()));

        let conn = db.conn();
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        conn.query_row(&sql, params_refs.as_slice(), Self::map_row)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "column".into(),
                    id: column_id.into(),
                },
                other => other.into(),
            })
    }

    /// Fetch a column by ID.
    pub fn get_by_id(db: &Db, column_id: &str) -> Result<Column, AppError> {
        db.conn()
            .query_row(
                "SELECT id, board_id, name, position, specialist_id, auto_trigger,
                        freeze_description, required_fields, required_artifact_types,
                        runtime_kind, created_at
                 FROM columns WHERE id = ?1",
                [column_id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "column".into(),
                    id: column_id.into(),
                },
                other => other.into(),
            })
    }

    /// List columns of a board, ordered by position ASC, id ASC.
    pub fn list_by_board(db: &Db, board_id: &str) -> Result<Vec<Column>, AppError> {
        let conn = db.conn();
        let mut stmt = conn.prepare(
            "SELECT id, board_id, name, position, specialist_id, auto_trigger,
                    freeze_description, required_fields, required_artifact_types,
                    runtime_kind, created_at
             FROM columns WHERE board_id = ?1
             ORDER BY position ASC, id ASC",
        )?;
        let rows: Vec<Column> = stmt
            .query_map([board_id], Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Hard delete a column. Cascades to tasks via the FK added in migration 005.
    ///
    /// Not currently exposed via the HTTP API (the spec only defines
    /// POST + PATCH for columns). Available for programmatic deletion
    /// (e.g., a future `KanbanService` cleanup path). Tested in
    /// `test_delete_cascades_tasks` to verify the migration 005 cascade
    /// works end-to-end.
    #[allow(dead_code)]
    pub fn delete(db: &Db, column_id: &str) -> Result<(), AppError> {
        let rows_affected = db
            .conn()
            .execute("DELETE FROM columns WHERE id = ?1", [column_id])?;
        if rows_affected == 0 {
            return Err(AppError::NotFound {
                resource: "column".into(),
                id: column_id.into(),
            });
        }
        Ok(())
    }

    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Column> {
        let auto_trigger_i: i64 = row.get(5)?;
        let freeze_i: i64 = row.get(6)?;
        let req_fields_json: String = row.get(7)?;
        let req_artifacts_json: String = row.get(8)?;
        let required_fields =
            parse_string_vec("required_fields", &req_fields_json).unwrap_or_default();
        let required_artifact_types =
            parse_string_vec("required_artifact_types", &req_artifacts_json).unwrap_or_default();
        Ok(Column {
            id: row.get(0)?,
            board_id: row.get(1)?,
            name: row.get(2)?,
            position: row.get(3)?,
            specialist_id: row.get(4)?,
            auto_trigger: auto_trigger_i != 0,
            freeze_description: freeze_i != 0,
            required_fields,
            required_artifact_types,
            runtime_kind: row.get(9)?,
            created_at: row.get(10)?,
        })
    }
}

/// Parse a JSON array of strings. Returns `None` for invalid JSON so
/// the caller can fall back to the default (`Vec::new()`). A `warn!`
/// is emitted on failure so corrupted rows don't silently disable
/// policy enforcement (e.g. a bad `required_fields` payload would
/// otherwise turn the column into a no-gate column without any
/// operator signal).
fn parse_string_vec(field: &'static str, json: &str) -> Option<Vec<String>> {
    match serde_json::from_str::<Vec<String>>(json) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(
                field,
                error = %e,
                "malformed JSON in column field; treating as empty \
                 (transition gates will not enforce this policy)"
            );
            None
        }
    }
}

/// Serialize a `&[String]` to a JSON array string. Wraps the
/// `serde_json::Error` in `AppError::Internal` so the four call sites
/// (create_tx + update × 2 fields) share one error-message wording.
fn json_string_vec(slice: &[String]) -> Result<String, AppError> {
    serde_json::to_string(slice)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("serialize required fields array: {e}")))
}

/// Validate that `auto_trigger=true` requires `specialist_id`.
///
/// Returns `AppError::Validation` with a clear message on failure.
pub(crate) fn validate_auto_trigger(
    auto_trigger: bool,
    specialist_id: Option<&str>,
) -> Result<(), AppError> {
    if auto_trigger && specialist_id.map(str::is_empty).unwrap_or(true) {
        return Err(AppError::validation(
            "auto_trigger requires a non-empty specialist_id",
        ));
    }
    Ok(())
}

/// Compute the next position for a column at the end of a board's columns.
///
/// Returns `max(position) + POSITION_STEP` over the same board.
pub fn next_position_in_column(conn: &Connection, board_id: &str) -> Result<i64, AppError> {
    let max: Option<i64> = conn
        .query_row(
            "SELECT MAX(position) FROM columns WHERE board_id = ?1",
            [board_id],
            |r| r.get(0),
        )
        .unwrap_or(None);
    Ok(max.unwrap_or(0) + POSITION_STEP)
}

/// Renumber all task positions within a column to `i * POSITION_STEP` spacing.
///
/// No-op when adjacent positions are already `>= MIN_GAP` apart. Called
/// from `TaskStore::move_to_column` after a cross-column move.
pub fn rebalance_column(conn: &Connection, column_id: &str) -> Result<(), AppError> {
    // Read all task positions in the column, ordered.
    let mut stmt = conn.prepare(
        "SELECT id, position FROM tasks WHERE column_id = ?1 ORDER BY position ASC, id ASC",
    )?;
    let rows: Vec<(String, i64)> = stmt
        .query_map([column_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    // Check whether rebalance is actually needed.
    let needs_rebalance = rows.windows(2).any(|w| (w[1].1 - w[0].1).abs() < MIN_GAP);
    if !needs_rebalance {
        return Ok(());
    }

    // Renumber as i * POSITION_STEP.
    for (i, (id, _)) in rows.iter().enumerate() {
        let new_pos = (i as i64 + 1) * POSITION_STEP;
        conn.execute(
            "UPDATE tasks SET position = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![new_pos, Utc::now().to_rfc3339(), id],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::seed_workspace_with_board;

    #[test]
    fn test_create_column_default_position() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, bid, _) = seed_workspace_with_board(&db);
        // Seed inserts 'test-col' at position 0, so the next default
        // position is 0 + POSITION_STEP (1000).
        let col = ColumnStore::create(
            &db, &bid, "To Do", None, None, false, None, None, None, None,
        )
        .unwrap();
        assert_eq!(col.name, "To Do");
        assert_eq!(col.position, 1000);
        assert!(!col.auto_trigger);
        assert!(col.specialist_id.is_none());
        assert!(!col.freeze_description);
        assert!(col.required_fields.is_empty());
        assert!(col.required_artifact_types.is_empty());
    }

    #[test]
    fn test_create_column_explicit_position() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, bid, _) = seed_workspace_with_board(&db);
        let col = ColumnStore::create(
            &db,
            &bid,
            "Done",
            Some(2048),
            None,
            false,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(col.position, 2048);
    }

    #[test]
    fn test_create_column_auto_trigger_requires_specialist() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, bid, _) = seed_workspace_with_board(&db);
        let result = ColumnStore::create(
            &db, &bid, "Broken", None, None, true, None, None, None, None,
        );
        assert!(matches!(
            result,
            Err(AppError::Validation { message: _, .. })
        ));
    }

    #[test]
    fn test_create_column_auto_trigger_with_specialist() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, bid, _) = seed_workspace_with_board(&db);
        let col = ColumnStore::create(
            &db,
            &bid,
            "Auto",
            None,
            Some("crafter"),
            true,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(col.auto_trigger);
        assert_eq!(col.specialist_id.as_deref(), Some("crafter"));
    }

    #[test]
    fn test_create_column_with_gate_fields() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, bid, _) = seed_workspace_with_board(&db);
        let req_fields = vec!["acceptance_criteria".to_string()];
        let req_artifacts = vec!["test_results".to_string()];
        let col = ColumnStore::create(
            &db,
            &bid,
            "Review",
            None,
            None,
            false,
            Some(true),
            Some(&req_fields),
            Some(&req_artifacts),
            None,
        )
        .unwrap();
        assert!(col.freeze_description);
        assert_eq!(col.required_fields, req_fields);
        assert_eq!(col.required_artifact_types, req_artifacts);
    }

    #[test]
    fn test_update_column_name() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, _bid, cid) = seed_workspace_with_board(&db);
        let updated = ColumnStore::update(
            &db,
            &cid,
            Some("Renamed"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.id, cid);
    }

    #[test]
    fn test_update_column_auto_trigger_toggle() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, _bid, cid) = seed_workspace_with_board(&db);
        // Turn on with specialist
        let updated = ColumnStore::update(
            &db,
            &cid,
            None,
            None,
            Some(Some("dev-crafter")),
            Some(true),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(updated.auto_trigger);
        // Turn off
        let updated = ColumnStore::update(
            &db,
            &cid,
            None,
            None,
            None,
            Some(false),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(!updated.auto_trigger);
    }

    #[test]
    fn test_update_column_clear_specialist_with_auto_trigger_fails() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, _bid, cid) = seed_workspace_with_board(&db);
        // First turn on with specialist
        ColumnStore::update(
            &db,
            &cid,
            None,
            None,
            Some(Some("crafter")),
            Some(true),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        // Now try to clear specialist while auto_trigger is still on
        let result = ColumnStore::update(
            &db,
            &cid,
            None,
            None,
            Some(None),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(matches!(
            result,
            Err(AppError::Validation { message: _, .. })
        ));
    }

    #[test]
    fn test_update_column_gate_fields() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, _bid, cid) = seed_workspace_with_board(&db);
        let req_fields = vec![
            "completion_summary".to_string(),
            "verification_report".to_string(),
        ];
        let updated = ColumnStore::update(
            &db,
            &cid,
            None,
            None,
            None,
            None,
            Some(true),
            Some(&req_fields),
            None,
            None,
        )
        .unwrap();
        assert!(updated.freeze_description);
        assert_eq!(updated.required_fields, req_fields);
    }

    #[test]
    fn test_list_by_board_ordered_by_position() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, bid, _) = seed_workspace_with_board(&db);
        // Seed inserts 'test-col' at position 0. Add 3 more at distinct positions.
        ColumnStore::create(
            &db,
            &bid,
            "C",
            Some(3_072),
            None,
            false,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        ColumnStore::create(
            &db,
            &bid,
            "A",
            Some(1_024),
            None,
            false,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        ColumnStore::create(
            &db,
            &bid,
            "B",
            Some(2_048),
            None,
            false,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let cols = ColumnStore::list_by_board(&db, &bid).unwrap();
        assert_eq!(cols.len(), 4);
        assert_eq!(cols[0].name, "test-col");
        assert_eq!(cols[1].name, "A");
        assert_eq!(cols[2].name, "B");
        assert_eq!(cols[3].name, "C");
    }

    #[test]
    fn test_delete_cascades_tasks() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, bid, cid) = seed_workspace_with_board(&db);
        // Insert a task
        let task_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Card', 0, 'active', ?4, ?4)",
                rusqlite::params![task_id, bid, cid, now],
            )
            .unwrap();
        // Delete the column — task should be cascade-deleted (migration 005)
        ColumnStore::delete(&db, &cid).unwrap();
        let count: i32 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM tasks WHERE id = ?1", [task_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "task should be cascade-deleted with column");
    }

    #[test]
    fn test_rebalance_column_no_op_when_positions_far_apart() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, bid, cid) = seed_workspace_with_board(&db);
        // Insert 3 tasks with positions 1000, 2000, 3000
        for (i, pos) in [1000i64, 2000, 3000].iter().enumerate() {
            db.conn()
                .execute(
                    "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?6)",
                    rusqlite::params![
                        uuid::Uuid::new_v4().to_string(),
                        bid,
                        cid,
                        format!("T{}", i + 1),
                        pos,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .unwrap();
        }
        db.with_transaction(|conn| rebalance_column(conn, &cid))
            .unwrap();
        // Positions unchanged
        let positions: Vec<i64> = db
            .conn()
            .prepare("SELECT position FROM tasks WHERE column_id = ?1 ORDER BY position ASC")
            .unwrap()
            .query_map([&cid], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(positions, vec![1000, 2000, 3000]);
    }

    #[test]
    fn test_rebalance_column_renumbers_when_too_close() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, bid, cid) = seed_workspace_with_board(&db);
        // Insert 3 tasks with positions 1000, 1001, 1002 — gap < MIN_GAP
        for (i, pos) in [1000i64, 1001, 1002].iter().enumerate() {
            db.conn()
                .execute(
                    "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?6)",
                    rusqlite::params![
                        uuid::Uuid::new_v4().to_string(),
                        bid,
                        cid,
                        format!("T{}", i + 1),
                        pos,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .unwrap();
        }
        db.with_transaction(|conn| rebalance_column(conn, &cid))
            .unwrap();
        let positions: Vec<i64> = db
            .conn()
            .prepare("SELECT position FROM tasks WHERE column_id = ?1 ORDER BY position ASC")
            .unwrap()
            .query_map([&cid], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(positions, vec![1000, 2000, 3000]);
    }

    #[test]
    fn test_columns_migration_adds_runtime_kind() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, bid, _) = seed_workspace_with_board(&db);
        // Create a column without runtime_kind — it should be NULL.
        let col = ColumnStore::create(&db, &bid, "Test", None, None, false, None, None, None, None)
            .unwrap();
        assert!(col.runtime_kind.is_none());

        // Create a column with runtime_kind — it should be persisted.
        let col = ColumnStore::create(
            &db,
            &bid,
            "CLI Lane",
            None,
            None,
            false,
            None,
            None,
            None,
            Some("claude-code"),
        )
        .unwrap();
        assert_eq!(col.runtime_kind.as_deref(), Some("claude-code"));

        // Update runtime_kind to a different value.
        let updated = ColumnStore::update(
            &db,
            &col.id,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Some("codex")),
        )
        .unwrap();
        assert_eq!(updated.runtime_kind.as_deref(), Some("codex"));

        // Clear runtime_kind.
        let updated = ColumnStore::update(
            &db,
            &col.id,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(None),
        )
        .unwrap();
        assert!(updated.runtime_kind.is_none());
    }
}
