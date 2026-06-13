//! `boards` store — CRUD for kanban boards.
//!
//! Boards belong to a workspace. Each board can optionally be created
//! with a template of columns (atomic insert via `Db::with_transaction`).
//!
//! All methods are stateless and take `&Db` as the first argument.

use rusqlite::Connection;
use serde::Serialize;
use uuid::Uuid;

use crate::db::Db;
use crate::error::AppError;
use crate::store::columns::{Column, ColumnStore};
use crate::store::tasks::{Task, TaskStore};
use chrono::Utc;

/// Domain representation of a board row.
#[derive(Debug, Clone, Serialize)]
pub struct Board {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub created_at: String,
}

/// Composite response for `GET /api/boards/:id`.
///
/// Columns are ordered by `position ASC, id ASC`; tasks are flat across
/// the board (the client groups by `column_id`).
#[derive(Debug, Serialize)]
pub struct BoardDetail {
    pub board: Board,
    pub columns: Vec<Column>,
    pub tasks: Vec<Task>,
}

/// Spec for a column created as part of a board template.
///
/// The three transition-gate fields (`freeze_description`,
/// `required_fields`, `required_artifact_types`) default to no-op so
/// existing callers (HTTP API, default board template) can ignore
/// them. `Default` is derived so test fixtures can use
/// `..Default::default()` to leave them unset.
#[derive(Debug, Clone)]
pub struct NewColumnSpec<'a> {
    pub name: &'a str,
    pub position: Option<i64>,
    pub specialist_id: Option<&'a str>,
    pub auto_trigger: bool,
    pub freeze_description: bool,
    pub required_fields: Vec<String>,
    pub required_artifact_types: Vec<String>,
    pub runtime_kind: Option<&'a str>,
    pub stage: crate::store::columns::ColumnStage,
}

impl<'a> Default for NewColumnSpec<'a> {
    fn default() -> Self {
        Self {
            name: "",
            position: None,
            specialist_id: None,
            auto_trigger: false,
            freeze_description: false,
            required_fields: Vec::new(),
            required_artifact_types: Vec::new(),
            runtime_kind: None,
            stage: crate::store::columns::ColumnStage::Dev,
        }
    }
}

/// Stateless store for board persistence.
pub struct BoardStore;

impl BoardStore {
    /// Insert a new board, optionally with template columns, atomically.
    ///
    /// The board row + all template columns are inserted in a single
    /// transaction. If any column insert fails (e.g., the auto-trigger
    /// guard rejects a `NewColumnSpec` with `auto_trigger=true` and no
    /// `specialist_id`), the entire board creation rolls back.
    pub fn create(
        db: &Db,
        workspace_id: &str,
        name: &str,
        template: &[NewColumnSpec<'_>],
    ) -> Result<Board, AppError> {
        db.with_transaction(|conn| {
            let board = Self::create_tx(conn, workspace_id, name)?;
            for spec in template {
                ColumnStore::create_tx(
                    conn,
                    &board.id,
                    spec.name,
                    spec.position,
                    spec.specialist_id,
                    spec.auto_trigger,
                    Some(spec.freeze_description),
                    Some(&spec.required_fields),
                    Some(&spec.required_artifact_types),
                    spec.runtime_kind,
                    spec.stage,
                    None,
                )?;
            }
            Ok(board)
        })
    }

    /// Insert a board row inside an existing transaction.
    ///
    /// Used by `create` and reusable by future service-layer flows that
    /// need to atomically combine a board insert with other writes.
    pub fn create_tx(conn: &Connection, workspace_id: &str, name: &str) -> Result<Board, AppError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        conn.query_row(
            "INSERT INTO boards (id, workspace_id, name, created_at)
             VALUES (?1, ?2, ?3, ?4)
             RETURNING id, workspace_id, name, created_at",
            rusqlite::params![id, workspace_id, name, now],
            Self::map_row,
        )
        .map_err(AppError::from)
    }

    /// Fetch a board by primary key. Caller is responsible for
    /// workspace authorization (this method does not scope by
    /// workspace — boards are visible across the workspace boundary
    /// only by ID, not by enumeration).
    pub fn get_by_id(db: &Db, board_id: &str) -> Result<Board, AppError> {
        db.conn()
            .query_row(
                "SELECT id, workspace_id, name, created_at FROM boards WHERE id = ?1",
                [board_id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "board".into(),
                    id: board_id.into(),
                },
                other => other.into(),
            })
    }

    /// Fetch a board by ID, but reject (as NotFound, to match the
    /// cross-workspace defense at `api/kanban.rs:179-186` and
    /// `tools/kanban/*`) when the board's workspace doesn't match
    /// the caller's. Returns the same `NotFound` shape as a missing
    /// board so an agent cannot enumerate boards across workspaces.
    pub fn get_in_workspace(
        db: &Db,
        board_id: &str,
        workspace_id: &str,
    ) -> Result<Board, AppError> {
        let board = Self::get_by_id(db, board_id)?;
        if board.workspace_id != workspace_id {
            return Err(AppError::NotFound {
                resource: "board".into(),
                id: board_id.into(),
            });
        }
        Ok(board)
    }

    /// Composite fetch: board + its columns + its tasks.
    ///
    /// Three roundtrips instead of one JOIN: each query is independently
    /// testable, and a future schema change to one entity touches one
    /// query. At kanban scale (≤200 cards/board) the latency difference
    /// is negligible.
    ///
    /// Workspace scoping: the board row carries `workspace_id`; the
    /// handler is expected to compare it to the requesting workspace.
    /// This store method does not enforce scoping — the API layer does.
    pub fn get_with_children(db: &Db, board_id: &str) -> Result<BoardDetail, AppError> {
        let board = Self::get_by_id(db, board_id)?;
        let columns = ColumnStore::list_by_board(db, &board.id)?;
        let tasks = TaskStore::list_by_board(db, &board.workspace_id, &board.id)?;
        Ok(BoardDetail {
            board,
            columns,
            tasks,
        })
    }

    /// List all boards in a workspace, ordered by name.
    pub fn list_by_workspace(db: &Db, workspace_id: &str) -> Result<Vec<Board>, AppError> {
        let conn = db.conn();
        let mut stmt = conn
            .prepare("SELECT id, workspace_id, name, created_at FROM boards WHERE workspace_id = ?1 ORDER BY name ASC, id ASC")?;
        let rows: Vec<Board> = stmt
            .query_map([workspace_id], Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Rename a board. Boards have no `updated_at` column.
    pub fn update_name(db: &Db, board_id: &str, new_name: &str) -> Result<Board, AppError> {
        db.conn()
            .query_row(
                "UPDATE boards SET name = ?1 WHERE id = ?2
                 RETURNING id, workspace_id, name, created_at",
                rusqlite::params![new_name, board_id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "board".into(),
                    id: board_id.into(),
                },
                other => other.into(),
            })
    }

    /// Hard delete a board. Cascades via FK constraints to columns and tasks.
    pub fn delete(db: &Db, board_id: &str) -> Result<(), AppError> {
        let rows_affected = db
            .conn()
            .execute("DELETE FROM boards WHERE id = ?1", [board_id])?;
        if rows_affected == 0 {
            return Err(AppError::NotFound {
                resource: "board".into(),
                id: board_id.into(),
            });
        }
        Ok(())
    }

    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Board> {
        Ok(Board {
            id: row.get(0)?,
            workspace_id: row.get(1)?,
            name: row.get(2)?,
            created_at: row.get(3)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::{make_test_db, seed_workspace_with_board};

    #[test]
    fn test_create_board_minimal() {
        let db = make_test_db();
        let (ws_id, _bid, _cid) = seed_workspace_with_board(&db);

        let board = BoardStore::create(&db, &ws_id, "My Board", &[]).unwrap();
        assert!(!board.id.is_empty());
        assert_eq!(board.workspace_id, ws_id);
        assert_eq!(board.name, "My Board");
        assert!(!board.created_at.is_empty());
    }

    #[test]
    fn test_create_board_with_template_columns_atomic() {
        let db = make_test_db();
        let (ws_id, _bid, _cid) = seed_workspace_with_board(&db);
        // New board with two template columns
        let template = [
            NewColumnSpec {
                name: "To Do",
                position: Some(0),
                ..Default::default()
            },
            NewColumnSpec {
                name: "Done",
                position: Some(1024),
                ..Default::default()
            },
        ];
        let board = BoardStore::create(&db, &ws_id, "Project", &template).unwrap();
        let columns = ColumnStore::list_by_board(&db, &board.id).unwrap();
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0].name, "To Do");
        assert_eq!(columns[1].name, "Done");
    }

    #[test]
    fn test_create_board_template_rolls_back_on_invalid_column() {
        // auto_trigger=true with no specialist_id must reject (and roll back
        // the board insert so no orphan board is left behind).
        let db = make_test_db();
        let (ws_id, _, _) = seed_workspace_with_board(&db);

        let template = [NewColumnSpec {
            name: "Broken",
            position: Some(0),
            auto_trigger: true,
            ..Default::default()
        }];
        let result = BoardStore::create(&db, &ws_id, "Should Roll Back", &template);
        assert!(result.is_err());

        // No "Should Roll Back" board should exist.
        let count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM boards WHERE name = 'Should Roll Back'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "board should have been rolled back");
    }

    #[test]
    fn test_get_with_children_returns_board_columns_tasks() {
        let db = make_test_db();
        let (_ws_id, bid, cid) = seed_workspace_with_board(&db);
        // Insert a second column + a task
        let col2_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO columns (id, board_id, name, position, created_at)
                 VALUES (?1, ?2, 'col-2', 1024, ?3)",
                rusqlite::params![col2_id, bid, now],
            )
            .unwrap();
        let task_id = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Card 1', 0, 'active', ?4, ?4)",
                rusqlite::params![task_id, bid, cid, now],
            )
            .unwrap();

        let detail = BoardStore::get_with_children(&db, &bid).unwrap();
        assert_eq!(detail.board.id, bid);
        assert_eq!(detail.columns.len(), 2);
        assert_eq!(detail.tasks.len(), 1);
        assert_eq!(detail.tasks[0].title, "Card 1");
    }

    #[test]
    fn test_get_with_children_wrong_board_returns_not_found() {
        let db = make_test_db();
        let result = BoardStore::get_with_children(&db, "nonexistent");
        assert!(matches!(result, Err(AppError::NotFound { resource: r, .. }) if r == "board"));
    }

    #[test]
    fn test_list_by_workspace_scoped() {
        let db = make_test_db();
        let (ws_id, _, _) = seed_workspace_with_board(&db);
        let boards = BoardStore::list_by_workspace(&db, &ws_id).unwrap();
        // The seed function inserts one board.
        assert_eq!(boards.len(), 1);
        // Other workspace sees nothing.
        let empty = BoardStore::list_by_workspace(&db, "other-ws").unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_update_name() {
        let db = make_test_db();
        let (_, bid, _) = seed_workspace_with_board(&db);
        let updated = BoardStore::update_name(&db, &bid, "Renamed").unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.id, bid);
    }

    #[test]
    fn test_update_name_not_found() {
        let db = make_test_db();
        let result = BoardStore::update_name(&db, "nonexistent", "X");
        assert!(matches!(result, Err(AppError::NotFound { resource: r, .. }) if r == "board"));
    }

    #[test]
    fn test_delete() {
        let db = make_test_db();
        let (_, bid, _) = seed_workspace_with_board(&db);
        BoardStore::delete(&db, &bid).unwrap();
        let result = BoardStore::get_by_id(&db, &bid);
        assert!(matches!(result, Err(AppError::NotFound { .. })));
    }

    #[test]
    fn test_delete_cascades_columns_and_tasks() {
        let db = make_test_db();
        let (_, bid, cid) = seed_workspace_with_board(&db);
        // Insert a task in the column
        let task_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Card', 0, 'active', ?4, ?4)",
                rusqlite::params![task_id, bid, cid, now],
            )
            .unwrap();

        BoardStore::delete(&db, &bid).unwrap();

        let col_count: i32 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM columns WHERE id = ?1", [cid], |r| {
                r.get(0)
            })
            .unwrap();
        let task_count: i32 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM tasks WHERE id = ?1", [task_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(col_count, 0, "column should be cascade-deleted");
        assert_eq!(task_count, 0, "task should be cascade-deleted");
    }

    #[test]
    fn test_default_board_template() {
        let db = make_test_db();
        let (ws_id, _bid, _cid) = seed_workspace_with_board(&db);

        let template = [
            NewColumnSpec {
                name: "Backlog",
                position: Some(0),
                specialist_id: Some("backlog-refiner"),
                auto_trigger: true,
                ..Default::default()
            },
            NewColumnSpec {
                name: "To Do",
                position: Some(1000),
                specialist_id: Some("todo-orchestrator"),
                auto_trigger: true,
                ..Default::default()
            },
            NewColumnSpec {
                name: "In Progress",
                position: Some(2000),
                specialist_id: Some("dev-crafter"),
                auto_trigger: true,
                ..Default::default()
            },
            NewColumnSpec {
                name: "Review",
                position: Some(3000),
                specialist_id: Some("review-guard"),
                auto_trigger: true,
                ..Default::default()
            },
            NewColumnSpec {
                name: "Done",
                position: Some(4000),
                specialist_id: Some("done-reporter"),
                auto_trigger: false,
                ..Default::default()
            },
        ];

        let board = BoardStore::create(&db, &ws_id, "Sprint 1", &template).unwrap();
        assert_eq!(board.name, "Sprint 1", "board name should match");

        let columns = ColumnStore::list_by_board(&db, &board.id).unwrap();

        assert_eq!(columns.len(), 5, "default board must have 5 columns");

        // Verify column names, specialist bindings, and auto-trigger flags.
        let expected: Vec<(&str, &str, bool)> = vec![
            ("Backlog", "backlog-refiner", true),
            ("To Do", "todo-orchestrator", true),
            ("In Progress", "dev-crafter", true),
            ("Review", "review-guard", true),
            ("Done", "done-reporter", false),
        ];

        for (i, (name, specialist_id, auto_trigger)) in expected.iter().enumerate() {
            assert_eq!(columns[i].name, *name, "column[{i}] name");
            assert_eq!(
                columns[i].specialist_id.as_deref(),
                Some(*specialist_id),
                "column[{i}] specialist_id"
            );
            assert_eq!(
                columns[i].auto_trigger, *auto_trigger,
                "column[{i}] auto_trigger"
            );
            // feat-028 transition-gate fields default to no-op for all 5 columns.
            assert!(
                !columns[i].freeze_description,
                "column[{i}] freeze_description should default to false"
            );
            assert!(
                columns[i].required_fields.is_empty(),
                "column[{i}] required_fields should default to []"
            );
            assert!(
                columns[i].required_artifact_types.is_empty(),
                "column[{i}] required_artifact_types should default to []"
            );
        }

        // Verify position ordering is ascending.
        for i in 1..columns.len() {
            assert!(
                columns[i].position > columns[i - 1].position,
                "column positions must be ascending: {} > {}",
                columns[i].position,
                columns[i - 1].position
            );
        }
    }

    #[test]
    fn test_default_board_specialists_load_from_disk() {
        let specialists_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/specialists");
        let mut registry = crate::specialist::SpecialistRegistry::new();
        let (loaded, skipped) = registry.load_from_dir(&specialists_dir);

        assert_eq!(
            loaded, 5,
            "must load 5 specialists from resources/specialists/"
        );
        assert_eq!(skipped, 0, "no specialist files should be skipped");

        let expected: Vec<(&str, &str, &str)> = vec![
            ("backlog-refiner", "planning", "Turns rough cards"),
            ("todo-orchestrator", "planning", "Validates stories"),
            ("dev-crafter", "implementation", "Implements changes"),
            ("review-guard", "review", "Independently verifies"),
            ("done-reporter", "reporting", "Writes completion"),
        ];

        for (name, profile, desc_prefix) in &expected {
            let specialist = registry
                .get_by_name(name)
                .unwrap_or_else(|| panic!("specialist '{name}' not found"));
            assert_eq!(
                specialist.tool_profile.as_deref(),
                Some(*profile),
                "{name} tool_profile"
            );
            assert!(
                specialist.description.starts_with(desc_prefix),
                "{name} description starts with '{desc_prefix}', got '{}'",
                specialist.description
            );
            assert!(
                !specialist.system_prompt.is_empty(),
                "{name} system_prompt must not be empty"
            );
            assert!(
                specialist.tags.contains(&"kanban".to_string()),
                "{name} must have 'kanban' tag"
            );
        }
    }
}
