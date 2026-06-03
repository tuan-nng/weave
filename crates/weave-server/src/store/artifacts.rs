//! `artifacts` store — CRUD for task-attached evidence (feat-031).
//!
//! An artifact is a `(task_id, type, content)` triple an agent attaches
//! to a kanban task. The same `(task_id, type)` UNIQUE index powers
//! `provide`'s upsert and `list_by_task`'s per-task lookup.
//!
//! Workspace scoping: every read/write that takes a `workspace_id`
//! JOINs through `tasks` → `boards.workspace_id` so an agent in
//! workspace A cannot read or mutate artifacts of a task in
//! workspace B. The two cross-workspace escape hatches are:
//!
//!   1. `list_by_task` and `get_by_id` return `NotFound` when the
//!      underlying task lives in a different workspace.
//!   2. `create` and `provide` reject with the same `NotFound` when
//!      the task's workspace does not match.
//!
//! All SQL is parameterized; the only string interpolation is in the
//! `provide` upsert, which builds the `ON CONFLICT` clause from a
//! fixed set of column names (not user input).
//!
//! No `delete` method: cleanup flows through `tasks(id) ON DELETE
//! CASCADE` (migration 007). The store exposes no surface to remove
//! a single artifact.

#[cfg(test)]
use rusqlite::Connection;

use std::collections::HashSet;

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::db::Db;
use crate::error::AppError;

/// Domain representation of an artifact row.
///
/// The Rust field is named `type_` to avoid the `type` keyword. The
/// on-disk column is `type` (see migration 007). The JSON wire format
/// is `"type"` (per `serde(rename = "type")`).
#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    pub id: String,
    pub task_id: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Stateless store for artifact persistence.
pub struct ArtifactStore;

/// Columns selected from the artifacts table, qualified with the
/// `artifacts` alias so JOINs against `tasks` and `boards` don't
/// produce "ambiguous column name" errors.
const SELECT_COLS: &str = "a.id, a.task_id, a.type, a.content, a.created_at, a.updated_at";

impl ArtifactStore {
    /// Insert a new artifact row. Fails with `Conflict` if
    /// `(task_id, type)` already exists; fails with `NotFound` if
    /// `task_id` does not belong to `workspace_id`.
    pub fn create(
        db: &Db,
        task_id: &str,
        artifact_type: &str,
        content: &str,
        workspace_id: &str,
    ) -> Result<Artifact, AppError> {
        verify_task_in_workspace(db, task_id, workspace_id)?;
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        db.conn()
            .query_row(
                "INSERT INTO artifacts (id, task_id, type, content, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 RETURNING id, task_id, type, content, created_at, updated_at",
                rusqlite::params![id, task_id, artifact_type, content, now],
                Self::map_row,
            )
            .map_err(|e| {
                crate::db::map_insert_error(
                    e,
                    "an artifact with the same task_id and type already exists",
                    "task",
                )
            })
    }

    /// Insert-or-replace by `(task_id, type)`. If a row exists,
    /// `content` is replaced and `updated_at` is bumped; `id` and
    /// `created_at` are preserved. Returns the row either way.
    ///
    /// The `ON CONFLICT(task_id, type) DO UPDATE` clause is the
    /// single-statement atomic upsert; no read-then-write race.
    pub fn provide(
        db: &Db,
        task_id: &str,
        artifact_type: &str,
        content: &str,
        workspace_id: &str,
    ) -> Result<Artifact, AppError> {
        verify_task_in_workspace(db, task_id, workspace_id)?;
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        // ON CONFLICT must use the unique index from migration 007.
        // Returning the full row from both branches lets the caller
        // treat the operation as "give me the row, regardless of
        // whether it was just created or just updated".
        db.conn()
            .query_row(
                "INSERT INTO artifacts (id, task_id, type, content, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(task_id, type) DO UPDATE SET
                     content = excluded.content,
                     updated_at = excluded.updated_at
                 RETURNING id, task_id, type, content, created_at, updated_at",
                rusqlite::params![id, task_id, artifact_type, content, now],
                Self::map_row,
            )
            .map_err(AppError::from)
    }

    /// Fetch one artifact by id, workspace-scoped. Cross-workspace
    /// access returns `NotFound` (defense-in-depth — agents cannot
    /// enumerate artifacts across workspaces).
    #[allow(dead_code)] // not currently called by a tool; available for future tool/API use
    pub fn get_by_id(db: &Db, artifact_id: &str, workspace_id: &str) -> Result<Artifact, AppError> {
        db.conn()
            .query_row(
                &format!(
                    "SELECT {SELECT_COLS}
                     FROM artifacts a
                     JOIN tasks t ON t.id = a.task_id
                     JOIN boards b ON b.id = t.board_id
                     WHERE a.id = ?1 AND b.workspace_id = ?2"
                ),
                rusqlite::params![artifact_id, workspace_id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "artifact".into(),
                    id: artifact_id.into(),
                },
                other => other.into(),
            })
    }

    /// List a task's artifacts, optionally filtered by type.
    /// Workspace-scoped via JOIN through `tasks` → `boards`. Ordered
    /// by `created_at ASC, id ASC` for stable pagination. Capped at
    /// `DEFAULT_LIST_LIMIT` (500) to match `TaskStore::list`.
    pub fn list_by_task(
        db: &Db,
        task_id: &str,
        workspace_id: &str,
        artifact_type: Option<&str>,
    ) -> Result<Vec<Artifact>, AppError> {
        verify_task_in_workspace(db, task_id, workspace_id)?;
        let mut sql = format!(
            "SELECT {SELECT_COLS}
             FROM artifacts a
             JOIN tasks t ON t.id = a.task_id
             JOIN boards b ON b.id = t.board_id
             WHERE a.task_id = ?1 AND b.workspace_id = ?2"
        );
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(task_id.to_string()),
            Box::new(workspace_id.to_string()),
        ];
        if let Some(t) = artifact_type {
            sql.push_str(" AND a.type = ?3");
            params.push(Box::new(t.to_string()));
        }
        sql.push_str(" ORDER BY a.created_at ASC, a.id ASC LIMIT 500");

        let conn = db.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let rows: Vec<Artifact> = stmt
            .query_map(params_refs.as_slice(), Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Return the set of artifact types currently present on a task,
    /// workspace-scoped. Used by `service::kanban::check_transition_gates`
    /// so the gate stays a pure function over its inputs. Returns an
    /// empty set when the task has no artifacts (the common case).
    pub fn list_types_for_task(
        db: &Db,
        task_id: &str,
        workspace_id: &str,
    ) -> Result<HashSet<String>, AppError> {
        let conn = db.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT a.type
             FROM artifacts a
             JOIN tasks t ON t.id = a.task_id
             JOIN boards b ON b.id = t.board_id
             WHERE a.task_id = ?1 AND b.workspace_id = ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![task_id, workspace_id], |r| {
            r.get::<_, String>(0)
        })?;
        let set: HashSet<String> = rows.collect::<Result<HashSet<_>, _>>()?;
        Ok(set)
    }

    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
        Ok(Artifact {
            id: row.get(0)?,
            task_id: row.get(1)?,
            type_: row.get(2)?,
            content: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    }
}

/// Helper for the gate tests in `service::kanban::tests`. Exposed at
/// the module level (not inside the `#[cfg(test)] mod tests` block) so
/// it's reachable via the canonical path
/// `crate::store::artifacts::seed_artifact_row`. Compiled only under
/// `#[cfg(test)]` so it does not appear in production builds.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn seed_artifact_row(
    conn: &Connection,
    task_id: &str,
    artifact_type: &str,
    content: &str,
) {
    conn.execute(
        "INSERT INTO artifacts (id, task_id, type, content, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        rusqlite::params![
            Uuid::new_v4().to_string(),
            task_id,
            artifact_type,
            content,
            Utc::now().to_rfc3339(),
        ],
    )
    .unwrap();
}

/// Cross-workspace defense shared by every workspace-scoped method.
/// Returns `NotFound` when the task does not exist OR lives on a
/// board in a different workspace (the two cases are intentionally
/// indistinguishable to prevent enumeration).
fn verify_task_in_workspace(db: &Db, task_id: &str, workspace_id: &str) -> Result<(), AppError> {
    let exists: Option<String> = db
        .conn()
        .query_row(
            "SELECT t.id FROM tasks t
             JOIN boards b ON b.id = t.board_id
             WHERE t.id = ?1 AND b.workspace_id = ?2",
            rusqlite::params![task_id, workspace_id],
            |r| r.get(0),
        )
        .ok();
    if exists.is_some() {
        Ok(())
    } else {
        Err(AppError::NotFound {
            resource: "task".into(),
            id: task_id.into(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::{make_test_db, seed_workspace_with_board};

    const TEST_WS: &str = "test-workspace";

    /// Seed: workspace → board → column → task. Returns
    /// `(workspace_id, board_id, column_id, task_id)`.
    fn seed_with_task(db: &Db) -> (String, String, String, String) {
        let (ws, bid, cid) = seed_workspace_with_board(db);
        let task_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'T', 0, 'active', ?4, ?4)",
                rusqlite::params![task_id, bid, cid, now],
            )
            .unwrap();
        (ws, bid, cid, task_id)
    }

    #[test]
    fn test_create_persists_row() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        let a = ArtifactStore::create(&db, &task_id, "screenshot", "img-bytes", &ws).unwrap();
        assert_eq!(a.task_id, task_id);
        assert_eq!(a.type_, "screenshot");
        assert_eq!(a.content, "img-bytes");
        assert!(!a.id.is_empty());
        assert_eq!(a.created_at, a.updated_at);
    }

    #[test]
    fn test_create_empty_content_is_allowed() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        // `request_artifact` semantics: an empty-content placeholder.
        let a = ArtifactStore::create(&db, &task_id, "screenshot", "", &ws).unwrap();
        assert_eq!(a.content, "");
    }

    #[test]
    fn test_create_duplicate_type_returns_conflict() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        ArtifactStore::create(&db, &task_id, "screenshot", "first", &ws).unwrap();
        let err = ArtifactStore::create(&db, &task_id, "screenshot", "second", &ws).unwrap_err();
        match err {
            AppError::Conflict(msg) => assert!(msg.contains("already exists"), "got: {msg}"),
            other => panic!("expected Conflict, got: {other:?}"),
        }
    }

    #[test]
    fn test_create_unknown_task_returns_not_found() {
        let db = make_test_db();
        let err =
            ArtifactStore::create(&db, "no-such-task", "screenshot", "", TEST_WS).unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "task"));
    }

    #[test]
    fn test_create_cross_workspace_returns_not_found() {
        let db = make_test_db();
        let (_ws, _bid, _cid, task_id) = seed_with_task(&db);
        let err = ArtifactStore::create(&db, &task_id, "screenshot", "", "other-ws").unwrap_err();
        assert!(matches!(err, AppError::NotFound { .. }));
    }

    #[test]
    fn test_provide_creates_when_missing() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        let a = ArtifactStore::provide(&db, &task_id, "log", "line 1", &ws).unwrap();
        assert_eq!(a.content, "line 1");
        assert!(!a.id.is_empty());
    }

    #[test]
    fn test_provide_updates_when_present_preserves_id() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        let first = ArtifactStore::provide(&db, &task_id, "log", "v1", &ws).unwrap();
        // The two `provide` calls use `Utc::now().to_rfc3339()` which
        // has second-precision. A same-second call would silently
        // make the `updated_at` bump unobservable. Sleep just past
        // one second to guarantee the bump is visible.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let second = ArtifactStore::provide(&db, &task_id, "log", "v2", &ws).unwrap();
        assert_eq!(first.id, second.id, "id must be preserved across upsert");
        assert_eq!(
            first.created_at, second.created_at,
            "created_at must not change"
        );
        assert_eq!(second.content, "v2");
        assert!(
            second.updated_at > first.updated_at,
            "updated_at must be bumped on upsert (first={}, second={})",
            first.updated_at,
            second.updated_at
        );
    }

    #[test]
    fn test_provide_does_not_affect_other_types() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        ArtifactStore::provide(&db, &task_id, "log", "v1", &ws).unwrap();
        ArtifactStore::provide(&db, &task_id, "screenshot", "img", &ws).unwrap();
        // Upserting "log" must not touch the "screenshot" row.
        ArtifactStore::provide(&db, &task_id, "log", "v2", &ws).unwrap();
        let all = ArtifactStore::list_by_task(&db, &task_id, &ws, None).unwrap();
        assert_eq!(all.len(), 2);
        let log = all.iter().find(|a| a.type_ == "log").unwrap();
        assert_eq!(log.content, "v2");
    }

    #[test]
    fn test_get_by_id_returns_row() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        let created = ArtifactStore::create(&db, &task_id, "log", "x", &ws).unwrap();
        let fetched = ArtifactStore::get_by_id(&db, &created.id, &ws).unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.type_, "log");
    }

    #[test]
    fn test_get_by_id_cross_workspace_returns_not_found() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        let created = ArtifactStore::create(&db, &task_id, "log", "x", &ws).unwrap();
        let err = ArtifactStore::get_by_id(&db, &created.id, "other-ws").unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "artifact"));
    }

    #[test]
    fn test_get_by_id_unknown_id_returns_not_found() {
        let db = make_test_db();
        let err = ArtifactStore::get_by_id(&db, "no-such-id", TEST_WS).unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "artifact"));
    }

    #[test]
    fn test_list_by_task_returns_all() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        ArtifactStore::create(&db, &task_id, "log", "a", &ws).unwrap();
        ArtifactStore::create(&db, &task_id, "screenshot", "b", &ws).unwrap();
        ArtifactStore::create(&db, &task_id, "test_results", "c", &ws).unwrap();
        let rows = ArtifactStore::list_by_task(&db, &task_id, &ws, None).unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn test_list_by_task_with_type_filter() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        ArtifactStore::create(&db, &task_id, "log", "a", &ws).unwrap();
        ArtifactStore::create(&db, &task_id, "screenshot", "b", &ws).unwrap();
        let rows = ArtifactStore::list_by_task(&db, &task_id, &ws, Some("screenshot")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].type_, "screenshot");
    }

    #[test]
    fn test_list_by_task_unknown_task_returns_not_found() {
        let db = make_test_db();
        let err = ArtifactStore::list_by_task(&db, "no-such-task", TEST_WS, None).unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "task"));
    }

    #[test]
    fn test_list_by_task_empty() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        let rows = ArtifactStore::list_by_task(&db, &task_id, &ws, None).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn test_list_types_for_task_returns_set() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        ArtifactStore::create(&db, &task_id, "log", "a", &ws).unwrap();
        ArtifactStore::create(&db, &task_id, "screenshot", "b", &ws).unwrap();
        ArtifactStore::provide(&db, &task_id, "log", "a-v2", &ws).unwrap(); // upsert
        let types = ArtifactStore::list_types_for_task(&db, &task_id, &ws).unwrap();
        assert_eq!(types.len(), 2);
        assert!(types.contains("log"));
        assert!(types.contains("screenshot"));
    }

    #[test]
    fn test_list_types_for_task_empty() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        let types = ArtifactStore::list_types_for_task(&db, &task_id, &ws).unwrap();
        assert!(types.is_empty());
    }

    #[test]
    fn test_list_types_for_task_cross_workspace_returns_empty_set() {
        // `list_types_for_task` scopes via the SQL JOIN on
        // `b.workspace_id = ?`. A foreign workspace returns an empty
        // set (read-side behavior matches `list_by_task`'s
        // empty-list shape; the explicit `verify_task_in_workspace`
        // in `list_by_task` is the reason that one returns
        // NotFound — see `test_list_by_task_unknown_task_returns_not_found`).
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        ArtifactStore::create(&db, &task_id, "log", "a", &ws).unwrap();
        let types = ArtifactStore::list_types_for_task(&db, &task_id, "other-ws").unwrap();
        assert!(types.is_empty(), "cross-workspace read must yield no types");
    }

    #[test]
    fn test_task_deletion_cascades_artifacts() {
        let db = make_test_db();
        let (ws, _bid, _cid, task_id) = seed_with_task(&db);
        ArtifactStore::create(&db, &task_id, "log", "a", &ws).unwrap();
        ArtifactStore::create(&db, &task_id, "screenshot", "b", &ws).unwrap();
        // Delete the task — artifacts must follow via FK cascade.
        db.conn()
            .execute("DELETE FROM tasks WHERE id = ?1", [&task_id])
            .unwrap();
        let count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM artifacts WHERE task_id = ?1",
                [&task_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "ON DELETE CASCADE must remove artifacts");
    }
}
