use chrono::Utc;
use rusqlite::Connection;
use serde::Serialize;
use uuid::Uuid;

use crate::db::Db;
use crate::error::AppError;
use crate::store::columns::rebalance_column;

/// Valid task statuses for the kanban-level task lifecycle.
///
/// These are the values stored in `tasks.status` and accepted by
/// both the HTTP PATCH endpoint and the agent tool `update_task_status`.
pub(crate) const VALID_TASK_STATUSES: &[&str] = &["active", "done", "archived"];

/// Validate a user-supplied task status. Returns the same
/// `AppError::Validation` message the HTTP API uses; canonical for
/// every call site (HTTP handlers, agent tools, store methods).
pub(crate) fn validate_status(s: &str) -> Result<(), AppError> {
    if !VALID_TASK_STATUSES.contains(&s) {
        return Err(AppError::validation(format!(
            "invalid task status '{}'; valid values: {}",
            s,
            VALID_TASK_STATUSES.join(", ")
        )));
    }
    Ok(())
}

/// Default maximum number of tasks returned by `list`.
const DEFAULT_LIST_LIMIT: u32 = 500;

/// Domain representation of a task row.
#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: String,
    pub board_id: String,
    pub column_id: String,
    pub title: String,
    pub description: Option<String>,
    pub position: i64,
    pub status: String,
    pub session_id: Option<String>,
    pub acceptance_criteria: Option<String>,
    pub completion_summary: Option<String>,
    pub verification_report: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Fields that can be updated by agents via `update_task_fields`.
///
/// Narrow, agent-facing: only the three context columns. The HTTP
/// PATCH endpoint uses the wider `UpdateTask` struct below.
pub struct UpdateTaskFields {
    pub acceptance_criteria: Option<String>,
    pub completion_summary: Option<String>,
    pub verification_report: Option<String>,
}

/// All editable task fields. Used by `PATCH /api/tasks/:tid`.
///
/// Fields use `Option<T>` to mean "if present, set to this value";
/// nullable fields use `Option<Option<T>>` so the caller can
/// distinguish "field absent" (`None`) from "field present and null"
/// (`Some(None)`).
#[derive(Clone)]
pub struct UpdateTask {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub column_id: Option<String>,
    pub position: Option<i64>,
    pub status: Option<String>,
    pub session_id: Option<Option<String>>,
    pub acceptance_criteria: Option<Option<String>>,
    pub completion_summary: Option<Option<String>>,
    pub verification_report: Option<Option<String>>,
}

/// Stateless store for task persistence.
///
/// All methods take `&Db` — no connection pooling, no lifetime management.
/// All queries are workspace-scoped via a JOIN through `boards`.
pub struct TaskStore;

/// Columns selected from the tasks table (aliased as `t` for JOINs).
const SELECT_COLS: &str = "t.id, t.board_id, t.column_id, t.title, t.description, \
    t.position, t.status, t.session_id, t.acceptance_criteria, t.completion_summary, \
    t.verification_report, t.created_at, t.updated_at";

/// Columns for RETURNING clause (no table alias needed).
const RETURNING_COLS: &str = "id, board_id, column_id, title, description, \
    position, status, session_id, acceptance_criteria, completion_summary, \
    verification_report, created_at, updated_at";

impl TaskStore {
    /// Insert a new task. `position` is auto-assigned when `None` (max+POSITION_STEP).
    ///
    /// Returns `Validation` if `column_id` does not belong to `board_id`.
    /// This prevents cross-board and cross-workspace task placement.
    pub fn create(
        db: &Db,
        board_id: &str,
        column_id: &str,
        title: &str,
        description: Option<&str>,
        position: Option<i64>,
        status: Option<&str>,
    ) -> Result<Task, AppError> {
        let status = status.unwrap_or("active");
        validate_status(status)?;
        if !column_belongs_to_board(db, column_id, board_id)? {
            return Err(AppError::validation(format!(
                "column '{}' does not belong to board '{}'",
                column_id, board_id
            )));
        }
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        let pos = match position {
            Some(p) => p,
            None => {
                // next position in column
                let max: Option<i64> = db
                    .conn()
                    .query_row(
                        "SELECT MAX(position) FROM tasks WHERE column_id = ?1",
                        [column_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(None);
                max.unwrap_or(0) + 1000
            }
        };

        db.conn()
            .query_row(
                "INSERT INTO tasks (id, board_id, column_id, title, description, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 RETURNING id, board_id, column_id, title, description, position, status, session_id, acceptance_criteria, completion_summary, verification_report, created_at, updated_at",
                rusqlite::params![id, board_id, column_id, title, description, pos, status, now],
                Self::map_row,
            )
            .map_err(AppError::from)
    }

    /// Hard delete a task. Workspace-scoped to prevent cross-workspace deletes.
    pub fn delete(db: &Db, task_id: &str, workspace_id: &str) -> Result<(), AppError> {
        let rows_affected = db.conn().execute(
            "DELETE FROM tasks WHERE id = ?1 AND board_id IN (SELECT id FROM boards WHERE workspace_id = ?2)",
            rusqlite::params![task_id, workspace_id],
        )?;
        if rows_affected == 0 {
            return Err(AppError::NotFound {
                resource: "task".into(),
                id: task_id.into(),
            });
        }
        Ok(())
    }

    /// Fetch a task by primary key, scoped to a workspace.
    pub fn get_by_id(db: &Db, task_id: &str, workspace_id: &str) -> Result<Task, AppError> {
        db.conn()
            .query_row(
                &format!(
                    "SELECT {SELECT_COLS}
                     FROM tasks t
                     JOIN boards b ON b.id = t.board_id
                     WHERE t.id = ?1 AND b.workspace_id = ?2"
                ),
                rusqlite::params![task_id, workspace_id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "task".into(),
                    id: task_id.into(),
                },
                other => other.into(),
            })
    }

    /// List tasks with optional filters, scoped to a workspace.
    ///
    /// All filters are AND-ed. Returns matching tasks ordered by position ASC, id ASC.
    /// Results are capped at `DEFAULT_LIST_LIMIT` rows.
    ///
    /// `query` is a free-text substring matched against `title` and
    /// `description` (case-insensitive via SQLite's default `LIKE`
    /// collation for ASCII). Empty/whitespace `query` is treated as `None`.
    /// The query is bound as a parameter — `LIKE` wildcards (`%`, `_`) in
    /// the user input are treated as wildcards (intentional, matches `grep` UX).
    #[allow(clippy::too_many_arguments)]
    pub fn list(
        db: &Db,
        workspace_id: &str,
        board_id: Option<&str>,
        column_id: Option<&str>,
        status: Option<&str>,
        query: Option<&str>,
    ) -> Result<Vec<Task>, AppError> {
        let mut sql = format!(
            "SELECT {SELECT_COLS}
             FROM tasks t
             JOIN boards b ON b.id = t.board_id
             WHERE b.workspace_id = ?1"
        );
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        params.push(Box::new(workspace_id.to_string()));
        let mut idx = 2u32;

        if let Some(bid) = board_id {
            sql.push_str(&format!(" AND t.board_id = ?{idx}"));
            params.push(Box::new(bid.to_string()));
            idx += 1;
        }
        if let Some(cid) = column_id {
            sql.push_str(&format!(" AND t.column_id = ?{idx}"));
            params.push(Box::new(cid.to_string()));
            idx += 1;
        }
        if let Some(s) = status {
            sql.push_str(&format!(" AND t.status = ?{idx}"));
            params.push(Box::new(s.to_string()));
            idx += 1;
        }
        if let Some(q) = query.filter(|q| !q.trim().is_empty()) {
            sql.push_str(&format!(
                " AND (t.title LIKE ?{idx} OR t.description LIKE ?{idx})"
            ));
            params.push(Box::new(format!("%{}%", q)));
        }

        sql.push_str(&format!(
            " ORDER BY t.position ASC, t.id ASC LIMIT {DEFAULT_LIST_LIMIT}"
        ));

        let conn = db.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let rows: Vec<Task> = stmt
            .query_map(params_refs.as_slice(), Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// List all tasks on a board, scoped to a workspace.
    ///
    /// Convenience over `list` with `board_id` filter; used by
    /// `BoardStore::get_with_children`.
    pub fn list_by_board(
        db: &Db,
        workspace_id: &str,
        board_id: &str,
    ) -> Result<Vec<Task>, AppError> {
        Self::list(db, workspace_id, Some(board_id), None, None, None)
    }

    /// List active tasks in `workspace_id` that are not currently
    /// bound to a session (feat-053).
    ///
    /// Used by the new-session wizard's Step 4 "attach to a task"
    /// picker. The "unbound + active" predicate is the canonical
    /// "this task is ready for someone to pick it up" signal in
    /// the kanban model: a task is `active` until done/archived,
    /// and the lane-automation flow binds a session via
    /// `tasks.session_id` when a card moves into an
    /// `auto_trigger=true` column. A `NULL` `session_id` means
    /// the card is sitting in the queue, not currently in flight.
    ///
    /// Equivalent SQL:
    ///   SELECT t.* FROM tasks t
    ///   JOIN boards b ON b.id = t.board_id
    ///   WHERE b.workspace_id = ?1
    ///     AND t.status = 'active'
    ///     AND t.session_id IS NULL
    ///   ORDER BY t.position ASC, t.id ASC
    ///   LIMIT 500
    ///
    /// Hard Constraint #5 (workspace-scoping) is enforced via the
    /// JOIN. `DEFAULT_LIST_LIMIT` (500) caps the result; a future
    /// pagination follow-up can add `?limit=&offset=` when a single
    /// workspace ever holds more than 500 unbound active tasks.
    pub fn list_unbound_in_workspace(db: &Db, workspace_id: &str) -> Result<Vec<Task>, AppError> {
        let conn = db.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SELECT_COLS}
             FROM tasks t
             JOIN boards b ON b.id = t.board_id
             WHERE b.workspace_id = ?1
               AND t.status = 'active'
               AND t.session_id IS NULL
             ORDER BY t.position ASC, t.id ASC
             LIMIT {DEFAULT_LIST_LIMIT}"
        ))?;
        let rows: Vec<Task> = stmt
            .query_map([workspace_id], Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Update task status with validation, scoped to a workspace.
    ///
    /// Validates against `VALID_TASK_STATUSES` before hitting the database.
    pub fn update_status(
        db: &Db,
        task_id: &str,
        workspace_id: &str,
        new_status: &str,
    ) -> Result<Task, AppError> {
        validate_status(new_status)?;

        let now = Utc::now().to_rfc3339();

        db.conn()
            .query_row(
                &format!(
                    "UPDATE tasks SET status = ?1, updated_at = ?2
                     WHERE id = ?3 AND board_id IN (
                         SELECT id FROM boards WHERE workspace_id = ?4
                     )
                     RETURNING {RETURNING_COLS}"
                ),
                rusqlite::params![new_status, now, task_id, workspace_id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "task".into(),
                    id: task_id.into(),
                },
                other => other.into(),
            })
    }

    /// Update the three context columns, scoped to a workspace.
    ///
    /// Only non-None fields are written. If all fields are None, returns the
    /// task unchanged (no-op). Used by the agent tool `update_task_fields`.
    pub fn update_fields(
        db: &Db,
        task_id: &str,
        workspace_id: &str,
        fields: &UpdateTaskFields,
    ) -> Result<Task, AppError> {
        if fields.acceptance_criteria.is_none()
            && fields.completion_summary.is_none()
            && fields.verification_report.is_none()
        {
            return Self::get_by_id(db, task_id, workspace_id);
        }

        let now = Utc::now().to_rfc3339();
        let mut sets = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1u32;

        if let Some(ref ac) = fields.acceptance_criteria {
            sets.push(format!("acceptance_criteria = ?{idx}"));
            params.push(Box::new(ac.clone()));
            idx += 1;
        }
        if let Some(ref cs) = fields.completion_summary {
            sets.push(format!("completion_summary = ?{idx}"));
            params.push(Box::new(cs.clone()));
            idx += 1;
        }
        if let Some(ref vr) = fields.verification_report {
            sets.push(format!("verification_report = ?{idx}"));
            params.push(Box::new(vr.clone()));
            idx += 1;
        }

        sets.push(format!("updated_at = ?{idx}"));
        params.push(Box::new(now));
        idx += 1;

        let task_id_idx = idx;
        let ws_id_idx = idx + 1;

        let sql = format!(
            "UPDATE tasks SET {}
             WHERE id = ?{task_id_idx} AND board_id IN (
                 SELECT id FROM boards WHERE workspace_id = ?{ws_id_idx}
             )
             RETURNING {RETURNING_COLS}",
            sets.join(", ")
        );
        params.push(Box::new(task_id.to_string()));
        params.push(Box::new(workspace_id.to_string()));

        let conn = db.conn();
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        conn.query_row(&sql, params_refs.as_slice(), Self::map_row)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "task".into(),
                    id: task_id.into(),
                },
                other => other.into(),
            })
    }

    /// Generic partial update for all editable task fields. Used by
    /// `PATCH /api/tasks/:tid`.
    ///
    /// Only fields that are `Some` (and `Some(Some(v))` for nullable
    /// fields) are written. Returns `Validation` if `status` is present
    /// but not in `VALID_TASK_STATUSES`.
    ///
    /// `column_id` is rejected — callers must use `move_to_column` so the
    /// same-board invariant and position rebalance are honored.
    pub fn update(
        db: &Db,
        task_id: &str,
        workspace_id: &str,
        fields: &UpdateTask,
    ) -> Result<Task, AppError> {
        if fields.column_id.is_some() {
            return Err(AppError::validation(
                "use move_to_column to change a task's column; \
                 setting column_id via update is not allowed",
            ));
        }
        // No-op when nothing to change.
        if fields.title.is_none()
            && fields.description.is_none()
            && fields.column_id.is_none()
            && fields.position.is_none()
            && fields.status.is_none()
            && fields.session_id.is_none()
            && fields.acceptance_criteria.is_none()
            && fields.completion_summary.is_none()
            && fields.verification_report.is_none()
        {
            return Self::get_by_id(db, task_id, workspace_id);
        }

        if let Some(s) = fields.status.as_deref() {
            validate_status(s)?;
        }

        let now = Utc::now().to_rfc3339();
        let mut sets = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1u32;

        if let Some(ref t) = fields.title {
            sets.push(format!("title = ?{idx}"));
            params.push(Box::new(t.clone()));
            idx += 1;
        }
        if let Some(ref desc) = fields.description {
            sets.push(format!("description = ?{idx}"));
            params.push(Box::new(desc.clone()));
            idx += 1;
        }
        if let Some(p) = fields.position {
            sets.push(format!("position = ?{idx}"));
            params.push(Box::new(p));
            idx += 1;
        }
        if let Some(ref s) = fields.status {
            sets.push(format!("status = ?{idx}"));
            params.push(Box::new(s.clone()));
            idx += 1;
        }
        if let Some(ref sid) = fields.session_id {
            sets.push(format!("session_id = ?{idx}"));
            params.push(Box::new(sid.clone()));
            idx += 1;
        }
        if let Some(ref ac) = fields.acceptance_criteria {
            sets.push(format!("acceptance_criteria = ?{idx}"));
            params.push(Box::new(ac.clone()));
            idx += 1;
        }
        if let Some(ref cs) = fields.completion_summary {
            sets.push(format!("completion_summary = ?{idx}"));
            params.push(Box::new(cs.clone()));
            idx += 1;
        }
        if let Some(ref vr) = fields.verification_report {
            sets.push(format!("verification_report = ?{idx}"));
            params.push(Box::new(vr.clone()));
            idx += 1;
        }

        sets.push(format!("updated_at = ?{idx}"));
        params.push(Box::new(now));
        idx += 1;

        let task_id_idx = idx;
        let ws_id_idx = idx + 1;

        let sql = format!(
            "UPDATE tasks SET {}
             WHERE id = ?{task_id_idx} AND board_id IN (
                 SELECT id FROM boards WHERE workspace_id = ?{ws_id_idx}
             )
             RETURNING {RETURNING_COLS}",
            sets.join(", ")
        );
        params.push(Box::new(task_id.to_string()));
        params.push(Box::new(workspace_id.to_string()));

        let conn = db.conn();
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        conn.query_row(&sql, params_refs.as_slice(), Self::map_row)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "task".into(),
                    id: task_id.into(),
                },
                other => other.into(),
            })
    }

    /// Move a task to a new column, optionally updating its position.
    /// Triggers a position rebalance inside the same transaction.
    pub fn move_to_column(
        db: &Db,
        task_id: &str,
        workspace_id: &str,
        target_column_id: &str,
        new_position: Option<i64>,
    ) -> Result<Task, AppError> {
        db.with_transaction(|conn| {
            Self::move_to_column_tx(conn, task_id, workspace_id, target_column_id, new_position)
        })
    }

    /// `move_to_column` variant for callers that already hold a `&Connection`
    /// (e.g., composing with other writes in a larger transaction).
    ///
    /// Returns `Validation` if the target column does not belong to the
    /// task's current board. This prevents cross-board and cross-workspace
    /// task placement.
    pub fn move_to_column_tx(
        conn: &Connection,
        task_id: &str,
        workspace_id: &str,
        target_column_id: &str,
        new_position: Option<i64>,
    ) -> Result<Task, AppError> {
        // Verify the target column belongs to the task's current board.
        let task_board_id: String = conn
            .query_row(
                "SELECT t.board_id FROM tasks t
                 JOIN boards b ON b.id = t.board_id
                 WHERE t.id = ?1 AND b.workspace_id = ?2",
                rusqlite::params![task_id, workspace_id],
                |r| r.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "task".into(),
                    id: task_id.into(),
                },
                other => other.into(),
            })?;
        let target_board_id: Option<String> = conn
            .query_row(
                "SELECT board_id FROM columns WHERE id = ?1",
                [target_column_id],
                |r| r.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "column".into(),
                    id: target_column_id.into(),
                },
                other => other.into(),
            })?;
        if target_board_id.as_deref() != Some(task_board_id.as_str()) {
            return Err(AppError::validation(format!(
                "column '{}' does not belong to board '{}'",
                target_column_id, task_board_id
            )));
        }

        let now = Utc::now().to_rfc3339();
        let pos = match new_position {
            Some(p) => p,
            None => {
                let max: Option<i64> = conn
                    .query_row(
                        "SELECT MAX(position) FROM tasks WHERE column_id = ?1",
                        [target_column_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(None);
                max.unwrap_or(0) + 1000
            }
        };

        let task = conn
            .query_row(
                &format!(
                    "UPDATE tasks SET column_id = ?1, position = ?2, updated_at = ?3
                     WHERE id = ?4 AND board_id IN (
                         SELECT id FROM boards WHERE workspace_id = ?5
                     )
                     RETURNING {RETURNING_COLS}"
                ),
                rusqlite::params![target_column_id, pos, now, task_id, workspace_id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "task".into(),
                    id: task_id.into(),
                },
                other => other.into(),
            })?;

        // Rebalance the target column so future inserts between neighbors
        // have a usable gap. This is O(N) on the column but N is small
        // (typical kanban columns have <50 cards).
        rebalance_column(conn, target_column_id)?;

        Ok(task)
    }

    /// Map a result row to a `Task`.
    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
        Ok(Task {
            id: row.get(0)?,
            board_id: row.get(1)?,
            column_id: row.get(2)?,
            title: row.get(3)?,
            description: row.get(4)?,
            position: row.get(5)?,
            status: row.get(6)?,
            session_id: row.get(7)?,
            acceptance_criteria: row.get(8)?,
            completion_summary: row.get(9)?,
            verification_report: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    }
}

/// Return `Ok(true)` if `column_id` exists and belongs to `board_id`,
/// `Ok(false)` if the column exists on a different board, or an error
/// if the column does not exist at all.
fn column_belongs_to_board(db: &Db, column_id: &str, board_id: &str) -> Result<bool, AppError> {
    let actual_board: Option<String> = db
        .conn()
        .query_row(
            "SELECT board_id FROM columns WHERE id = ?1",
            [column_id],
            |r| r.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                resource: "column".into(),
                id: column_id.into(),
            },
            other => other.into(),
        })?;
    Ok(actual_board.as_deref() == Some(board_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::{
        seed_workspace_with_board, seed_workspace_with_two_columns,
    };
    use std::path::Path;

    fn test_db() -> Db {
        Db::open(Path::new(":memory:")).expect("failed to open test db")
    }

    fn create_test_task(
        db: &Db,
        workspace_id: &str,
        board_id: &str,
        column_id: &str,
        title: &str,
        status: &str,
    ) -> Task {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?6)",
                rusqlite::params![id, board_id, column_id, title, status, now],
            )
            .unwrap();

        TaskStore::get_by_id(db, &id, workspace_id).unwrap()
    }

    #[test]
    fn test_get_by_id() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let created = create_test_task(&db, &ws, &board_id, &col_id, "Test task", "active");

        let fetched = TaskStore::get_by_id(&db, &created.id, &ws).unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.title, "Test task");
        assert_eq!(fetched.status, "active");
        assert_eq!(fetched.board_id, board_id);
        assert_eq!(fetched.column_id, col_id);
    }

    #[test]
    fn test_get_by_id_not_found() {
        let db = test_db();
        let (ws, _, _) = seed_workspace_with_board(&db);
        let result = TaskStore::get_by_id(&db, "nonexistent", &ws);
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { resource, id } => {
                assert_eq!(resource, "task");
                assert_eq!(id, "nonexistent");
            }
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_get_by_id_wrong_workspace() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Test task", "active");

        let result = TaskStore::get_by_id(&db, &task.id, "wrong-workspace");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { .. } => {}
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_list_no_filters() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        create_test_task(&db, &ws, &board_id, &col_id, "Task 1", "active");
        create_test_task(&db, &ws, &board_id, &col_id, "Task 2", "done");

        let tasks = TaskStore::list(&db, &ws, None, None, None, None).unwrap();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_list_filter_by_status() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        create_test_task(&db, &ws, &board_id, &col_id, "Task 1", "active");
        create_test_task(&db, &ws, &board_id, &col_id, "Task 2", "done");
        create_test_task(&db, &ws, &board_id, &col_id, "Task 3", "active");

        let tasks = TaskStore::list(&db, &ws, None, None, Some("active"), None).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|t| t.status == "active"));
    }

    #[test]
    fn test_list_filter_by_column() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        create_test_task(&db, &ws, &board_id, &col_id, "Task 1", "active");

        let tasks = TaskStore::list(&db, &ws, None, Some(&col_id), None, None).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].column_id, col_id);
    }

    #[test]
    fn test_list_filter_combined() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        create_test_task(&db, &ws, &board_id, &col_id, "Task 1", "active");
        create_test_task(&db, &ws, &board_id, &col_id, "Task 2", "done");

        let tasks = TaskStore::list(
            &db,
            &ws,
            Some(&board_id),
            Some(&col_id),
            Some("active"),
            None,
        )
        .unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Task 1");
    }

    #[test]
    fn test_list_empty() {
        let db = test_db();
        let (ws, _, _) = seed_workspace_with_board(&db);
        let tasks = TaskStore::list(&db, &ws, None, None, None, None).unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn test_list_scoped_to_workspace() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        create_test_task(&db, &ws, &board_id, &col_id, "Task 1", "active");

        let tasks = TaskStore::list(&db, "other-ws", None, None, None, None).unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn test_list_by_board() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        create_test_task(&db, &ws, &board_id, &col_id, "T1", "active");
        create_test_task(&db, &ws, &board_id, &col_id, "T2", "active");
        let tasks = TaskStore::list_by_board(&db, &ws, &board_id).unwrap();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_update_status() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Test task", "active");

        let updated = TaskStore::update_status(&db, &task.id, &ws, "done").unwrap();
        assert_eq!(updated.status, "done");
        assert_eq!(updated.id, task.id);
    }

    #[test]
    fn test_update_status_invalid() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Test task", "active");

        let result = TaskStore::update_status(&db, &task.id, &ws, "invalid_status");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("invalid task status"), "got: {}", msg);
            }
            other => panic!("expected Validation, got: {:?}", other),
        }
    }

    #[test]
    fn test_update_status_not_found() {
        let db = test_db();
        let (ws, _, _) = seed_workspace_with_board(&db);
        let result = TaskStore::update_status(&db, "nonexistent", &ws, "done");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { resource, .. } => assert_eq!(resource, "task"),
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_update_status_wrong_workspace() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Test task", "active");

        let result = TaskStore::update_status(&db, &task.id, "wrong-workspace", "done");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { .. } => {}
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_update_fields_completion_summary() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Test task", "active");

        let fields = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: Some("Did the thing".into()),
            verification_report: None,
        };
        let updated = TaskStore::update_fields(&db, &task.id, &ws, &fields).unwrap();
        assert_eq!(updated.completion_summary.as_deref(), Some("Did the thing"));
        assert!(updated.verification_report.is_none());
    }

    #[test]
    fn test_update_fields_verification_report() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Test task", "active");

        let fields = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: Some("Tests pass".into()),
        };
        let updated = TaskStore::update_fields(&db, &task.id, &ws, &fields).unwrap();
        assert_eq!(updated.verification_report.as_deref(), Some("Tests pass"));
        assert!(updated.completion_summary.is_none());
    }

    #[test]
    fn test_update_fields_both() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Test task", "active");

        let fields = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: Some("Did it".into()),
            verification_report: Some("Verified".into()),
        };
        let updated = TaskStore::update_fields(&db, &task.id, &ws, &fields).unwrap();
        assert_eq!(updated.completion_summary.as_deref(), Some("Did it"));
        assert_eq!(updated.verification_report.as_deref(), Some("Verified"));
    }

    #[test]
    fn test_update_fields_no_fields() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Test task", "active");

        let fields = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: None,
        };
        let updated = TaskStore::update_fields(&db, &task.id, &ws, &fields).unwrap();
        assert_eq!(updated.id, task.id);
        assert_eq!(updated.title, "Test task");
    }

    #[test]
    fn test_update_fields_not_found() {
        let db = test_db();
        let (ws, _, _) = seed_workspace_with_board(&db);
        let fields = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: Some("test".into()),
            verification_report: None,
        };
        let result = TaskStore::update_fields(&db, "nonexistent", &ws, &fields);
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { resource, .. } => assert_eq!(resource, "task"),
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_update_fields_wrong_workspace() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Test task", "active");

        let fields = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: Some("test".into()),
            verification_report: None,
        };
        let result = TaskStore::update_fields(&db, &task.id, "wrong-workspace", &fields);
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { .. } => {}
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_update_fields_partial_does_not_clobber() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Test task", "active");

        let fields1 = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: Some("Summary v1".into()),
            verification_report: None,
        };
        TaskStore::update_fields(&db, &task.id, &ws, &fields1).unwrap();

        let fields2 = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: Some("Report v1".into()),
        };
        let updated = TaskStore::update_fields(&db, &task.id, &ws, &fields2).unwrap();
        assert_eq!(updated.completion_summary.as_deref(), Some("Summary v1"));
        assert_eq!(updated.verification_report.as_deref(), Some("Report v1"));
    }

    // ---- feat-024: new methods ----

    #[test]
    fn test_create_task_default_position() {
        let db = test_db();
        let (_, board_id, col_id) = seed_workspace_with_board(&db);
        let task = TaskStore::create(&db, &board_id, &col_id, "New", None, None, None).unwrap();
        assert_eq!(task.title, "New");
        assert_eq!(task.status, "active");
        assert_eq!(task.position, 1000);
    }

    #[test]
    fn test_create_task_with_description_and_explicit_position() {
        let db = test_db();
        let (_, board_id, col_id) = seed_workspace_with_board(&db);
        let task = TaskStore::create(
            &db,
            &board_id,
            &col_id,
            "X",
            Some("desc"),
            Some(5000),
            Some("done"),
        )
        .unwrap();
        assert_eq!(task.position, 5000);
        assert_eq!(task.status, "done");
        assert_eq!(task.description.as_deref(), Some("desc"));
    }

    #[test]
    fn test_create_task_invalid_status() {
        let db = test_db();
        let (_, board_id, col_id) = seed_workspace_with_board(&db);
        let result = TaskStore::create(&db, &board_id, &col_id, "X", None, None, Some("bogus"));
        assert!(matches!(
            result,
            Err(AppError::Validation { message: _, .. })
        ));
    }

    #[test]
    fn test_delete_task() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "T", "active");
        TaskStore::delete(&db, &task.id, &ws).unwrap();
        let result = TaskStore::get_by_id(&db, &task.id, &ws);
        assert!(matches!(result, Err(AppError::NotFound { .. })));
    }

    #[test]
    fn test_delete_task_wrong_workspace() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "T", "active");
        let result = TaskStore::delete(&db, &task.id, "other-ws");
        assert!(matches!(result, Err(AppError::NotFound { .. })));
    }

    #[test]
    fn test_move_to_column() {
        let db = test_db();
        let (ws, bid, c1, c2) = seed_workspace_with_two_columns(&db);
        let task = create_test_task(&db, &ws, &bid, &c1, "Move me", "active");
        let moved = TaskStore::move_to_column(&db, &task.id, &ws, &c2, Some(2000)).unwrap();
        assert_eq!(moved.column_id, c2);
        assert_eq!(moved.position, 2000);
    }

    #[test]
    fn test_update_all_fields_via_patch() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Old", "active");

        // column_id changes must go through move_to_column (which also
        // updates position). After that, the rest of the fields are
        // applied via the regular update path.
        TaskStore::move_to_column(&db, &task.id, &ws, &col_id, Some(2048)).unwrap();

        let update = UpdateTask {
            title: Some("New title".into()),
            description: Some(Some("New desc".into())),
            column_id: None,
            position: None,
            status: Some("done".into()),
            session_id: Some(None),
            acceptance_criteria: Some(Some("AC".into())),
            completion_summary: Some(Some("CS".into())),
            verification_report: Some(Some("VR".into())),
        };
        let updated = TaskStore::update(&db, &task.id, &ws, &update).unwrap();
        assert_eq!(updated.title, "New title");
        assert_eq!(updated.description.as_deref(), Some("New desc"));
        assert_eq!(updated.position, 2048);
        assert_eq!(updated.status, "done");
        assert!(updated.session_id.is_none());
        assert_eq!(updated.acceptance_criteria.as_deref(), Some("AC"));
        assert_eq!(updated.completion_summary.as_deref(), Some("CS"));
        assert_eq!(updated.verification_report.as_deref(), Some("VR"));
    }

    #[test]
    fn test_update_no_fields_returns_unchanged() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "T", "active");
        let update = UpdateTask {
            title: None,
            description: None,
            column_id: None,
            position: None,
            status: None,
            session_id: None,
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: None,
        };
        let unchanged = TaskStore::update(&db, &task.id, &ws, &update).unwrap();
        assert_eq!(unchanged.id, task.id);
        assert_eq!(unchanged.title, "T");
    }

    #[test]
    fn test_update_invalid_status_returns_validation() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "T", "active");
        let update = UpdateTask {
            title: None,
            description: None,
            column_id: None,
            position: None,
            status: Some("bogus".into()),
            session_id: None,
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: None,
        };
        let result = TaskStore::update(&db, &task.id, &ws, &update);
        assert!(matches!(
            result,
            Err(AppError::Validation { message: _, .. })
        ));
    }

    #[test]
    fn test_update_partial_does_not_clobber() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);
        let task = create_test_task(&db, &ws, &board_id, &col_id, "Original", "active");
        // First set description
        let first = UpdateTask {
            title: None,
            description: Some(Some("first desc".into())),
            column_id: None,
            position: None,
            status: None,
            session_id: None,
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: None,
        };
        TaskStore::update(&db, &task.id, &ws, &first).unwrap();
        // Now only change title
        let second = UpdateTask {
            title: Some("Renamed".into()),
            description: None,
            column_id: None,
            position: None,
            status: None,
            session_id: None,
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: None,
        };
        let updated = TaskStore::update(&db, &task.id, &ws, &second).unwrap();
        assert_eq!(updated.title, "Renamed");
        assert_eq!(updated.description.as_deref(), Some("first desc"));
    }

    // ---- feat-053: list_unbound_in_workspace ----

    /// Inserts a task with an optional `session_id` directly via SQL
    /// (the `create_test_task` helper always sets `session_id = NULL`
    /// which is the wrong fixture for the unbound test). Returns the
    /// id so the test can assert on it. The caller is responsible for
    /// ensuring `session_id` (if `Some`) points to an existing row in
    /// `sessions` — `tasks.session_id` has `REFERENCES sessions(id)`,
    /// so a foreign-key violation otherwise aborts the insert.
    fn insert_task_with_session(
        db: &Db,
        board_id: &str,
        column_id: &str,
        title: &str,
        status: &str,
        session_id: Option<&str>,
    ) -> String {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, session_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?7)",
                rusqlite::params![id, board_id, column_id, title, status, session_id, now],
            )
            .unwrap();
        id
    }

    /// Inserts a minimal `sessions` row scoped to `workspace_id` so
    /// that a task can legally reference it. The session's other
    /// fields don't matter for the unbound-task query — we only need
    /// the FKs to `workspaces` and `providers` to exist. We reuse
    /// `kanban_test_helpers::seed_provider` to satisfy `provider_id`.
    fn insert_minimal_session(db: &Db, workspace_id: &str, session_id: &str) {
        let provider_id = crate::store::kanban_test_helpers::seed_provider(db);
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO sessions (id, workspace_id, provider_id, status, runtime_kind, mode, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'connecting', 'claude-code', 'wrapped', ?4, ?4)",
                rusqlite::params![session_id, workspace_id, provider_id, now],
            )
            .unwrap();
    }

    /// Only the (active, unbound) task is returned. The other two
    /// fixtures cover the reject paths: (active, bound) is "in
    /// flight" — a session has it; (done, unbound) is "finished" —
    /// a future pick-up would be wrong (the wizard should not offer
    /// closed work).
    #[test]
    fn test_list_unbound_tasks_returns_active_with_no_session() {
        let db = test_db();
        let (ws, board_id, col_id) = seed_workspace_with_board(&db);

        let unbound =
            insert_task_with_session(&db, &board_id, &col_id, "Pick me up", "active", None);
        // The bound fixture needs a real `sessions` row (FK). Seed
        // one with a fresh UUID and reference it.
        let bound_session = Uuid::new_v4().to_string();
        insert_minimal_session(&db, &ws, &bound_session);
        let _bound = insert_task_with_session(
            &db,
            &board_id,
            &col_id,
            "Already working on this",
            "active",
            Some(&bound_session),
        );
        let _done = insert_task_with_session(&db, &board_id, &col_id, "Finished", "done", None);

        let tasks = TaskStore::list_unbound_in_workspace(&db, &ws).unwrap();
        assert_eq!(tasks.len(), 1, "only active+unbound should be returned");
        assert_eq!(tasks[0].id, unbound);
        assert_eq!(tasks[0].status, "active");
        assert!(tasks[0].session_id.is_none());
    }

    /// Hard Constraint #5: every query includes `workspace_id`. A
    /// second workspace's tasks must never leak into the first
    /// workspace's response.
    #[test]
    fn test_list_unbound_tasks_excludes_other_workspace() {
        let db = test_db();
        let (ws_a, board_a, col_a) = seed_workspace_with_board(&db);
        // Seed a second workspace + board + column directly (the
        // helper only seeds the default workspace).
        let ws_b = Uuid::new_v4().to_string();
        let board_b = Uuid::new_v4().to_string();
        let col_b = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'ws-b', 'active', ?2, ?2)",
                rusqlite::params![ws_b, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at)
                 VALUES (?1, ?2, 'b-board', ?3)",
                rusqlite::params![board_b, ws_b, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO columns (id, board_id, name, position, created_at)
                 VALUES (?1, ?2, 'b-col', 0, ?3)",
                rusqlite::params![col_b, board_b, now],
            )
            .unwrap();

        insert_task_with_session(&db, &board_a, &col_a, "A task", "active", None);
        insert_task_with_session(&db, &board_b, &col_b, "B task", "active", None);

        let from_a = TaskStore::list_unbound_in_workspace(&db, &ws_a).unwrap();
        let from_b = TaskStore::list_unbound_in_workspace(&db, &ws_b).unwrap();
        assert_eq!(from_a.len(), 1);
        assert_eq!(from_b.len(), 1);
        assert_ne!(from_a[0].id, from_b[0].id);
    }
}
