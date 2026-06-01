use crate::db::Db;
use crate::error::AppError;
use chrono::Utc;
use serde::Serialize;

/// Valid task statuses for agent-facing task lifecycle.
///
/// These differ from the kanban-level `active/done/archived` values.
/// The tools validate against this set before hitting the database.
pub(crate) const VALID_TASK_STATUSES: &[&str] = &[
    "in_progress",
    "review_required",
    "completed",
    "needs_fix",
    "blocked",
];

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
pub struct UpdateTaskFields {
    pub acceptance_criteria: Option<String>,
    pub completion_summary: Option<String>,
    pub verification_report: Option<String>,
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
    pub fn list(
        db: &Db,
        workspace_id: &str,
        board_id: Option<&str>,
        column_id: Option<&str>,
        status: Option<&str>,
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

    /// Update task status with validation, scoped to a workspace.
    ///
    /// Validates against `VALID_TASK_STATUSES` before hitting the database.
    pub fn update_status(
        db: &Db,
        task_id: &str,
        workspace_id: &str,
        new_status: &str,
    ) -> Result<Task, AppError> {
        if !VALID_TASK_STATUSES.contains(&new_status) {
            return Err(AppError::Validation(format!(
                "invalid task status '{}'; valid values: {}",
                new_status,
                VALID_TASK_STATUSES.join(", ")
            )));
        }

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
    /// task unchanged (no-op).
    pub fn update_fields(
        db: &Db,
        task_id: &str,
        workspace_id: &str,
        fields: &UpdateTaskFields,
    ) -> Result<Task, AppError> {
        // If no fields to update, just return the current task
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

        // WHERE clause: task_id + workspace scoping
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const TEST_WS: &str = "test-ws-id";

    fn test_db() -> Db {
        Db::open(Path::new(":memory:")).expect("failed to open test db")
    }

    /// Seed a workspace, board, and column — required FK rows for creating tasks.
    fn seed_task_deps(db: &Db) -> (String, String) {
        let board_id = uuid::Uuid::new_v4().to_string();
        let col_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'test-ws', 'active', ?2, ?2)",
                rusqlite::params![TEST_WS, now],
            )
            .unwrap();

        db.conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at)
                 VALUES (?1, ?2, 'test-board', ?3)",
                rusqlite::params![board_id, TEST_WS, now],
            )
            .unwrap();

        db.conn()
            .execute(
                "INSERT INTO columns (id, board_id, name, position, created_at)
                 VALUES (?1, ?2, 'test-col', 0, ?3)",
                rusqlite::params![col_id, board_id, now],
            )
            .unwrap();

        (board_id, col_id)
    }

    /// Create a task with known values for testing.
    fn create_test_task(
        db: &Db,
        board_id: &str,
        column_id: &str,
        title: &str,
        status: &str,
    ) -> Task {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?6)",
                rusqlite::params![id, board_id, column_id, title, status, now],
            )
            .unwrap();

        TaskStore::get_by_id(db, &id, TEST_WS).unwrap()
    }

    #[test]
    fn test_get_by_id() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        let created = create_test_task(&db, &board_id, &col_id, "Test task", "in_progress");

        let fetched = TaskStore::get_by_id(&db, &created.id, TEST_WS).unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.title, "Test task");
        assert_eq!(fetched.status, "in_progress");
        assert_eq!(fetched.board_id, board_id);
        assert_eq!(fetched.column_id, col_id);
    }

    #[test]
    fn test_get_by_id_not_found() {
        let db = test_db();
        seed_task_deps(&db);
        let result = TaskStore::get_by_id(&db, "nonexistent", TEST_WS);
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
        let (board_id, col_id) = seed_task_deps(&db);
        let task = create_test_task(&db, &board_id, &col_id, "Test task", "in_progress");

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
        let (board_id, col_id) = seed_task_deps(&db);
        create_test_task(&db, &board_id, &col_id, "Task 1", "in_progress");
        create_test_task(&db, &board_id, &col_id, "Task 2", "completed");

        let tasks = TaskStore::list(&db, TEST_WS, None, None, None).unwrap();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_list_filter_by_status() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        create_test_task(&db, &board_id, &col_id, "Task 1", "in_progress");
        create_test_task(&db, &board_id, &col_id, "Task 2", "completed");
        create_test_task(&db, &board_id, &col_id, "Task 3", "in_progress");

        let tasks = TaskStore::list(&db, TEST_WS, None, None, Some("in_progress")).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|t| t.status == "in_progress"));
    }

    #[test]
    fn test_list_filter_by_column() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        create_test_task(&db, &board_id, &col_id, "Task 1", "in_progress");

        let tasks = TaskStore::list(&db, TEST_WS, None, Some(&col_id), None).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].column_id, col_id);
    }

    #[test]
    fn test_list_filter_combined() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        create_test_task(&db, &board_id, &col_id, "Task 1", "in_progress");
        create_test_task(&db, &board_id, &col_id, "Task 2", "completed");

        let tasks = TaskStore::list(
            &db,
            TEST_WS,
            Some(&board_id),
            Some(&col_id),
            Some("in_progress"),
        )
        .unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Task 1");
    }

    #[test]
    fn test_list_empty() {
        let db = test_db();
        seed_task_deps(&db);
        let tasks = TaskStore::list(&db, TEST_WS, None, None, None).unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn test_list_scoped_to_workspace() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        create_test_task(&db, &board_id, &col_id, "Task 1", "in_progress");

        // Different workspace sees nothing
        let tasks = TaskStore::list(&db, "other-ws", None, None, None).unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn test_update_status() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        let task = create_test_task(&db, &board_id, &col_id, "Test task", "in_progress");

        let updated = TaskStore::update_status(&db, &task.id, TEST_WS, "completed").unwrap();
        assert_eq!(updated.status, "completed");
        assert_eq!(updated.id, task.id);
    }

    #[test]
    fn test_update_status_invalid() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        let task = create_test_task(&db, &board_id, &col_id, "Test task", "in_progress");

        let result = TaskStore::update_status(&db, &task.id, TEST_WS, "invalid_status");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::Validation(msg) => {
                assert!(msg.contains("invalid task status"), "got: {}", msg);
            }
            other => panic!("expected Validation, got: {:?}", other),
        }
    }

    #[test]
    fn test_update_status_not_found() {
        let db = test_db();
        seed_task_deps(&db);
        let result = TaskStore::update_status(&db, "nonexistent", TEST_WS, "completed");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { resource, .. } => assert_eq!(resource, "task"),
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_update_status_wrong_workspace() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        let task = create_test_task(&db, &board_id, &col_id, "Test task", "in_progress");

        let result = TaskStore::update_status(&db, &task.id, "wrong-workspace", "completed");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { .. } => {}
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_update_fields_completion_summary() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        let task = create_test_task(&db, &board_id, &col_id, "Test task", "in_progress");

        let fields = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: Some("Did the thing".into()),
            verification_report: None,
        };
        let updated = TaskStore::update_fields(&db, &task.id, TEST_WS, &fields).unwrap();
        assert_eq!(updated.completion_summary.as_deref(), Some("Did the thing"));
        assert!(updated.verification_report.is_none());
    }

    #[test]
    fn test_update_fields_verification_report() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        let task = create_test_task(&db, &board_id, &col_id, "Test task", "in_progress");

        let fields = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: Some("Tests pass".into()),
        };
        let updated = TaskStore::update_fields(&db, &task.id, TEST_WS, &fields).unwrap();
        assert_eq!(updated.verification_report.as_deref(), Some("Tests pass"));
        assert!(updated.completion_summary.is_none());
    }

    #[test]
    fn test_update_fields_both() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        let task = create_test_task(&db, &board_id, &col_id, "Test task", "in_progress");

        let fields = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: Some("Did it".into()),
            verification_report: Some("Verified".into()),
        };
        let updated = TaskStore::update_fields(&db, &task.id, TEST_WS, &fields).unwrap();
        assert_eq!(updated.completion_summary.as_deref(), Some("Did it"));
        assert_eq!(updated.verification_report.as_deref(), Some("Verified"));
    }

    #[test]
    fn test_update_fields_no_fields() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        let task = create_test_task(&db, &board_id, &col_id, "Test task", "in_progress");

        let fields = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: None,
        };
        // No-op: returns the task unchanged
        let updated = TaskStore::update_fields(&db, &task.id, TEST_WS, &fields).unwrap();
        assert_eq!(updated.id, task.id);
        assert_eq!(updated.title, "Test task");
    }

    #[test]
    fn test_update_fields_not_found() {
        let db = test_db();
        seed_task_deps(&db);
        let fields = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: Some("test".into()),
            verification_report: None,
        };
        let result = TaskStore::update_fields(&db, "nonexistent", TEST_WS, &fields);
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { resource, .. } => assert_eq!(resource, "task"),
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_update_fields_wrong_workspace() {
        let db = test_db();
        let (board_id, col_id) = seed_task_deps(&db);
        let task = create_test_task(&db, &board_id, &col_id, "Test task", "in_progress");

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
        let (board_id, col_id) = seed_task_deps(&db);
        let task = create_test_task(&db, &board_id, &col_id, "Test task", "in_progress");

        // First update: set completion_summary
        let fields1 = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: Some("Summary v1".into()),
            verification_report: None,
        };
        TaskStore::update_fields(&db, &task.id, TEST_WS, &fields1).unwrap();

        // Second update: set verification_report only
        let fields2 = UpdateTaskFields {
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: Some("Report v1".into()),
        };
        let updated = TaskStore::update_fields(&db, &task.id, TEST_WS, &fields2).unwrap();

        // completion_summary should be unchanged
        assert_eq!(updated.completion_summary.as_deref(), Some("Summary v1"));
        assert_eq!(updated.verification_report.as_deref(), Some("Report v1"));
    }
}
