//! `codebases` store — CRUD for git repositories registered as codebases
//! within a workspace (feat-032).
//!
//! A codebase is a `(workspace_id, path)` row that names a git
//! working tree on disk. The `codebases.path` column is the absolute
//! filesystem path stored as-given; the API layer is responsible for
//! validating that the path is absolute on input. Sessions are
//! associated with codebases via the `sessions.cwd` prefix-match
//! helper `find_by_cwd_prefix` (see `system_design.md`).
//!
//! Workspace scoping: every read/write that takes a `workspace_id`
//! uses it as a SQL filter. The cross-workspace escape hatch
//! `get_in_workspace` returns `NotFound` when the row's workspace
//! doesn't match (matching the `boards` pattern at `store/boards.rs:134-147`).
//!
//! All SQL is parameterized; the only string interpolation is in the
//! `find_by_cwd_prefix` LIKE clause, which is fixed and never
//! includes user input.

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::db::Db;
use crate::error::AppError;

/// Domain representation of a codebase row.
#[derive(Debug, Clone, Serialize)]
pub struct Codebase {
    pub id: String,
    pub workspace_id: String,
    pub path: String,
    pub branch: Option<String>,
    pub label: Option<String>,
    pub created_at: String,
}

/// One recent commit (hash + first line of message).
#[derive(Debug, Clone, Serialize)]
pub struct GitCommit {
    pub hash: String,
    pub message: String,
}

/// Git status snapshot for the composite `GET /api/codebases/:id`.
///
/// `branch` is empty when the repo has no commits (porcelain header is
/// absent on an empty repo — the parser returns an empty string).
/// `dirty_files` is the union of `staged ∪ unstaged ∪ untracked` paths
/// from `git status --porcelain=v1 -b`. `recent_commits` is up to the
/// last 5 commits on HEAD.
#[derive(Debug, Clone, Serialize)]
pub struct GitStatus {
    pub branch: String,
    pub dirty_files: Vec<String>,
    pub recent_commits: Vec<GitCommit>,
}

/// Composite response for `GET /api/codebases/:id`.
///
/// `git_status` is `None` and `git_error` is `Some(msg)` when the path
/// does not exist or is not a git repo. This graceful-degrade shape
/// keeps the row visible to the client (so the user can edit or
/// delete it) instead of returning a 500 for a transient git failure.
#[derive(Debug, Serialize)]
pub struct CodebaseDetail {
    pub codebase: Codebase,
    pub git_status: Option<GitStatus>,
    pub git_error: Option<String>,
}

/// Stateless store for codebase persistence.
pub struct CodebaseStore;

/// Columns selected from the codebases table.
const SELECT_COLS: &str = "id, workspace_id, path, branch, label, created_at";

impl CodebaseStore {
    /// Insert a new codebase row. Fails with `Conflict` if
    /// `(workspace_id, path)` already exists (the UNIQUE index from
    /// migration 001).
    pub fn create(
        db: &Db,
        workspace_id: &str,
        path: &str,
        branch: Option<&str>,
        label: Option<&str>,
    ) -> Result<Codebase, AppError> {
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        db.conn()
            .query_row(
                &format!(
                    "INSERT INTO codebases ({SELECT_COLS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     RETURNING {SELECT_COLS}"
                ),
                rusqlite::params![id, workspace_id, path, branch, label, now],
                Self::map_row,
            )
            .map_err(|e| {
                crate::db::map_insert_error(
                    e,
                    "a codebase with the same path already exists in this workspace",
                    "workspace",
                )
            })
    }

    /// Fetch a codebase by primary key. No workspace scoping — callers
    /// that need cross-workspace defense must use `get_in_workspace`.
    pub fn get_by_id(db: &Db, codebase_id: &str) -> Result<Codebase, AppError> {
        db.conn()
            .query_row(
                &format!("SELECT {SELECT_COLS} FROM codebases WHERE id = ?1"),
                [codebase_id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "codebase".into(),
                    id: codebase_id.into(),
                },
                other => other.into(),
            })
    }

    /// Fetch a codebase by ID, but reject (as `NotFound`, to match
    /// the cross-workspace defense at `store/boards.rs:134-147`) when
    /// the codebase's workspace doesn't match the caller's. Returns
    /// the same `NotFound` shape as a missing codebase so an agent
    /// cannot enumerate codebases across workspaces.
    pub fn get_in_workspace(
        db: &Db,
        codebase_id: &str,
        workspace_id: &str,
    ) -> Result<Codebase, AppError> {
        let codebase = Self::get_by_id(db, codebase_id)?;
        if codebase.workspace_id != workspace_id {
            return Err(AppError::NotFound {
                resource: "codebase".into(),
                id: codebase_id.into(),
            });
        }
        Ok(codebase)
    }

    /// List all codebases in a workspace, ordered by `path ASC, id ASC`.
    pub fn list_by_workspace(db: &Db, workspace_id: &str) -> Result<Vec<Codebase>, AppError> {
        let conn = db.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SELECT_COLS} FROM codebases WHERE workspace_id = ?1
             ORDER BY path ASC, id ASC"
        ))?;
        let rows: Vec<Codebase> = stmt
            .query_map([workspace_id], Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Hard delete a codebase. Any `sessions.codebase_id` referencing
    /// this row is set to `NULL` (the FK declares `ON DELETE SET NULL`,
    /// migration 010); the session rows themselves survive with their
    /// `cwd` unchanged. The session's runtime sandbox stays active
    /// because the runtime falls back to `session.cwd` as the
    /// containment root whenever the binding is set — see
    /// `run_prompt_task` in `service::sessions`.
    pub fn delete(db: &Db, codebase_id: &str) -> Result<(), AppError> {
        let rows_affected = db
            .conn()
            .execute("DELETE FROM codebases WHERE id = ?1", [codebase_id])?;
        if rows_affected == 0 {
            return Err(AppError::NotFound {
                resource: "codebase".into(),
                id: codebase_id.into(),
            });
        }
        Ok(())
    }

    /// Find the longest matching codebase path for a given `cwd`.
    ///
    /// Returns the codebase whose registered `path` equals `cwd` or is
    /// a path-prefix of `cwd` AND has the longest `path` length (so
    /// nested worktrees resolve to the most specific codebase). Used
    /// by sessions to attribute work to a codebase at runtime;
    /// deferred wiring lives in `service::sessions` (see
    /// `system_design.md`).
    ///
    /// The trailing path separator in the prefix is intentional: a
    /// codebase at `/home/u/repo` must match `cwd` `/home/u/repo/foo`
    /// but NOT `/home/u/repo-other`. The SQL fragment is fixed (no
    /// user input) — `?1 = path OR ?1 LIKE path || '/%'` — so the
    /// literal `/` is safe.
    #[allow(dead_code)] // Kept for the future "cwd is a subdir of a registered codebase" use case.
    pub fn find_by_cwd_prefix(
        db: &Db,
        workspace_id: &str,
        cwd: &str,
    ) -> Result<Option<Codebase>, AppError> {
        // The literal `path` column value goes into the LIKE pattern
        // as a binding via concat — but the pattern pieces are
        // constants. The single user-controlled value is `cwd`, bound
        // as `?1`.
        //
        // Two matching conditions:
        //   1. `?1 = path`           — exact match (cwd IS the codebase)
        //   2. `?1 LIKE path || '/%'` — nested match (cwd is INSIDE the codebase)
        // The literal `'/'` in the concat is a constant, not user input.
        let conn = db.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SELECT_COLS} FROM codebases
             WHERE workspace_id = ?2
               AND (?1 = path OR ?1 LIKE path || '/%')
             ORDER BY length(path) DESC, id ASC
             LIMIT 1"
        ))?;
        let mut rows = stmt.query(rusqlite::params![cwd, workspace_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Self::map_row(row)?))
        } else {
            Ok(None)
        }
    }

    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Codebase> {
        Ok(Codebase {
            id: row.get(0)?,
            workspace_id: row.get(1)?,
            path: row.get(2)?,
            branch: row.get(3)?,
            label: row.get(4)?,
            created_at: row.get(5)?,
        })
    }
}

// ---------------------------------------------------------------------------
// Shared helpers (used by both the store and the API layer)
// ---------------------------------------------------------------------------

/// Compose a `CodebaseDetail` from a row + (optional) git snapshot.
///
/// The git snapshot is the result of a successful `compose_git_status`
/// call. Pass `Ok(Some(status))` for a happy path, `Ok(None)` for
/// "path is not a git repo" (the parser returned nothing), and
/// `Err(msg)` for any other failure. The function maps the three
/// outcomes to the `CodebaseDetail` shape described in the type docs.
pub fn build_detail(codebase: Codebase, git: Result<Option<GitStatus>, String>) -> CodebaseDetail {
    match git {
        Ok(Some(status)) => CodebaseDetail {
            codebase,
            git_status: Some(status),
            git_error: None,
        },
        Ok(None) => CodebaseDetail {
            codebase,
            git_status: None,
            git_error: Some("Path is not a git repository".to_string()),
        },
        Err(msg) => CodebaseDetail {
            codebase,
            git_status: None,
            git_error: Some(msg),
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::make_test_db;

    fn seed_workspace(db: &Db) -> String {
        crate::store::workspaces::WorkspaceStore::ensure_default(db).expect("ensure_default");
        db.conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .expect("default workspace")
    }

    #[test]
    fn test_create_persists_row() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let cb =
            CodebaseStore::create(&db, &ws, "/tmp/myrepo", Some("main"), Some("My repo")).unwrap();
        assert_eq!(cb.workspace_id, ws);
        assert_eq!(cb.path, "/tmp/myrepo");
        assert_eq!(cb.branch.as_deref(), Some("main"));
        assert_eq!(cb.label.as_deref(), Some("My repo"));
        assert!(!cb.id.is_empty());
        assert!(!cb.created_at.is_empty());
    }

    #[test]
    fn test_create_minimal_optionals_none() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let cb = CodebaseStore::create(&db, &ws, "/tmp/repo", None, None).unwrap();
        assert!(cb.branch.is_none());
        assert!(cb.label.is_none());
    }

    #[test]
    fn test_create_duplicate_path_in_workspace_returns_conflict() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        CodebaseStore::create(&db, &ws, "/tmp/dup", None, None).unwrap();
        let err = CodebaseStore::create(&db, &ws, "/tmp/dup", None, None).unwrap_err();
        match err {
            AppError::Conflict(msg) => {
                assert!(msg.contains("already exists"), "got: {msg}");
            }
            other => panic!("expected Conflict, got: {other:?}"),
        }
    }

    #[test]
    fn test_create_same_path_in_different_workspaces_is_allowed() {
        // The UNIQUE index is on (workspace_id, path), not path alone —
        // two workspaces may each register a codebase at the same path.
        let db = make_test_db();
        let ws1 = seed_workspace(&db);
        let ws2 = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, ?2, 'active', ?3, ?3)",
                rusqlite::params![ws2, "ws2", Utc::now().to_rfc3339()],
            )
            .unwrap();
        CodebaseStore::create(&db, &ws1, "/tmp/shared", None, None).unwrap();
        CodebaseStore::create(&db, &ws2, "/tmp/shared", None, None).unwrap();
    }

    #[test]
    fn test_get_by_id_returns_row() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let created = CodebaseStore::create(&db, &ws, "/tmp/x", None, None).unwrap();
        let fetched = CodebaseStore::get_by_id(&db, &created.id).unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.path, "/tmp/x");
    }

    #[test]
    fn test_get_by_id_unknown_returns_not_found() {
        let db = make_test_db();
        let err = CodebaseStore::get_by_id(&db, "nope").unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "codebase"));
    }

    #[test]
    fn test_get_in_workspace_cross_workspace_returns_not_found() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let created = CodebaseStore::create(&db, &ws, "/tmp/cb", None, None).unwrap();
        let err = CodebaseStore::get_in_workspace(&db, &created.id, "other-ws").unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "codebase"));
        // Same call from the right workspace succeeds.
        let ok = CodebaseStore::get_in_workspace(&db, &created.id, &ws).unwrap();
        assert_eq!(ok.id, created.id);
    }

    #[test]
    fn test_list_by_workspace_orders_by_path() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        CodebaseStore::create(&db, &ws, "/tmp/c", None, None).unwrap();
        CodebaseStore::create(&db, &ws, "/tmp/a", None, None).unwrap();
        CodebaseStore::create(&db, &ws, "/tmp/b", None, None).unwrap();
        let rows = CodebaseStore::list_by_workspace(&db, &ws).unwrap();
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["/tmp/a", "/tmp/b", "/tmp/c"]);
    }

    #[test]
    fn test_list_by_workspace_is_scoped() {
        let db = make_test_db();
        let ws1 = seed_workspace(&db);
        let ws2 = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, ?2, 'active', ?3, ?3)",
                rusqlite::params![ws2, "ws2", Utc::now().to_rfc3339()],
            )
            .unwrap();
        CodebaseStore::create(&db, &ws1, "/tmp/in1", None, None).unwrap();
        CodebaseStore::create(&db, &ws2, "/tmp/in2", None, None).unwrap();
        let in1 = CodebaseStore::list_by_workspace(&db, &ws1).unwrap();
        let in2 = CodebaseStore::list_by_workspace(&db, &ws2).unwrap();
        assert_eq!(in1.len(), 1);
        assert_eq!(in2.len(), 1);
        assert_eq!(in1[0].path, "/tmp/in1");
        assert_eq!(in2[0].path, "/tmp/in2");
    }

    #[test]
    fn test_list_by_workspace_empty() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let rows = CodebaseStore::list_by_workspace(&db, &ws).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn test_delete() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let cb = CodebaseStore::create(&db, &ws, "/tmp/del", None, None).unwrap();
        CodebaseStore::delete(&db, &cb.id).unwrap();
        let err = CodebaseStore::get_by_id(&db, &cb.id).unwrap_err();
        assert!(matches!(err, AppError::NotFound { .. }));
    }

    #[test]
    fn test_delete_unknown_returns_not_found() {
        let db = make_test_db();
        let err = CodebaseStore::delete(&db, "nope").unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "codebase"));
    }

    // ---- find_by_cwd_prefix ----

    #[test]
    fn test_find_by_cwd_prefix_returns_longest_match() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        CodebaseStore::create(&db, &ws, "/home/u", None, None).unwrap();
        let nested = CodebaseStore::create(&db, &ws, "/home/u/repo", None, None).unwrap();
        // The longer (nested) path wins over the shorter prefix.
        let hit = CodebaseStore::find_by_cwd_prefix(&db, &ws, "/home/u/repo/src").unwrap();
        assert_eq!(hit.map(|c| c.id), Some(nested.id));
    }

    #[test]
    fn test_find_by_cwd_prefix_exact_match() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let cb = CodebaseStore::create(&db, &ws, "/home/u/repo", None, None).unwrap();
        let hit = CodebaseStore::find_by_cwd_prefix(&db, &ws, "/home/u/repo").unwrap();
        assert_eq!(hit.map(|c| c.id), Some(cb.id));
    }

    #[test]
    fn test_find_by_cwd_prefix_no_match_returns_none() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        CodebaseStore::create(&db, &ws, "/home/u/repo", None, None).unwrap();
        let hit = CodebaseStore::find_by_cwd_prefix(&db, &ws, "/elsewhere/foo").unwrap();
        assert!(hit.is_none());
    }

    #[test]
    fn test_find_by_cwd_prefix_partial_segment_does_not_match() {
        // /home/u/repo must NOT match /home/u/repo-other (the path
        // separator in the LIKE pattern is what makes this safe).
        let db = make_test_db();
        let ws = seed_workspace(&db);
        CodebaseStore::create(&db, &ws, "/home/u/repo", None, None).unwrap();
        let hit = CodebaseStore::find_by_cwd_prefix(&db, &ws, "/home/u/repo-other").unwrap();
        assert!(hit.is_none());
    }

    #[test]
    fn test_find_by_cwd_prefix_scoped_to_workspace() {
        let db = make_test_db();
        let ws1 = seed_workspace(&db);
        let ws2 = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, ?2, 'active', ?3, ?3)",
                rusqlite::params![ws2, "ws2", Utc::now().to_rfc3339()],
            )
            .unwrap();
        CodebaseStore::create(&db, &ws2, "/tmp/in2", None, None).unwrap();
        // ws1 sees nothing even though ws2 has a matching path.
        let hit = CodebaseStore::find_by_cwd_prefix(&db, &ws1, "/tmp/in2").unwrap();
        assert!(hit.is_none());
    }

    // ---- build_detail ----

    #[test]
    fn test_build_detail_with_status() {
        let cb = Codebase {
            id: "c1".into(),
            workspace_id: "w1".into(),
            path: "/p".into(),
            branch: None,
            label: None,
            created_at: "t".into(),
        };
        let status = GitStatus {
            branch: "main".into(),
            dirty_files: vec!["a".into()],
            recent_commits: vec![],
        };
        let d = build_detail(cb, Ok(Some(status)));
        assert!(d.git_status.is_some());
        assert!(d.git_error.is_none());
    }

    #[test]
    fn test_build_detail_not_a_repo() {
        let cb = Codebase {
            id: "c1".into(),
            workspace_id: "w1".into(),
            path: "/p".into(),
            branch: None,
            label: None,
            created_at: "t".into(),
        };
        let d = build_detail(cb, Ok(None));
        assert!(d.git_status.is_none());
        assert!(d.git_error.as_deref() == Some("Path is not a git repository"));
    }

    #[test]
    fn test_build_detail_git_error() {
        let cb = Codebase {
            id: "c1".into(),
            workspace_id: "w1".into(),
            path: "/p".into(),
            branch: None,
            label: None,
            created_at: "t".into(),
        };
        let d = build_detail(cb, Err("git broken".into()));
        assert!(d.git_status.is_none());
        assert_eq!(d.git_error.as_deref(), Some("git broken"));
    }
}
