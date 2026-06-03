//! `notes` store — CRUD for workspace-scoped long-form notes (feat-030).
//!
//! A note is a `(workspace_id, title, type, content)` row an agent or
//! human keeps between sessions. Notes are not tied to a single task
//! — they live alongside the workspace so any agent that joins the
//! workspace can read them.
//!
//! Workspace scoping: every read/write that takes a `workspace_id`
//! uses it as a SQL filter. The cross-workspace defense in
//! `get_by_id` returns `NotFound` when the row's workspace doesn't
//! match the caller's (matching the `codebases::get_in_workspace`
//! pattern at `store/codebases.rs:127-140`).
//!
//! Title uniqueness is enforced at the schema level by the
//! `idx_notes_workspace_title` UNIQUE index from migration 008. The
//! `create` path maps the UNIQUE-violation error to `Conflict` via
//! `map_insert_error`; this keeps the store's error shape consistent
//! with `artifacts` and `codebases`.
//!
//! No `delete` method: cleanup flows through `workspaces(id) ON
//! DELETE CASCADE` (migration 008). The spec deliberately omits a
//! single-note delete tool — the only way a note leaves a workspace
//! is if the workspace itself does. If a future feature needs a
//! delete, add a `delete` method here and a corresponding tool, not
//! a direct API route.

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::db::Db;
use crate::error::AppError;

/// Whitelist of valid note types. Strict (not free-vocabulary like
/// `artifacts.type`) because notes are categorized at write time and
/// the `list_notes` filter expects to be able to compare against one
/// of these. Adding a new type here is a deliberate, versioned
/// change; the store's `validate_note_type` enforces membership.
pub(crate) const VALID_NOTE_TYPES: &[&str] = &["spec", "task", "general"];

/// Maximum title length, in chars. The cap sits between the
/// `workspaces.name` cap (100) and the `tasks.title` cap (500) to
/// keep titles compact enough for list-row display but roomy enough
/// for descriptive labels like "API endpoint contract (v2 draft)".
pub(crate) const MAX_NOTE_TITLE_LEN: usize = 200;

/// Domain representation of a note row.
///
/// The Rust field is named `type_` to avoid the `type` keyword. The
/// on-disk column is `type` (see migration 008). The JSON wire
/// format is `"type"` (per `serde(rename = "type")`).
#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Stateless store for note persistence.
pub struct NoteStore;

/// Validate that `s` is one of the whitelisted note types. Returns
/// `AppError::Validation` with a message naming the valid set so
/// the caller can correct the input without consulting the schema.
pub(crate) fn validate_note_type(s: &str) -> Result<&str, AppError> {
    if VALID_NOTE_TYPES.contains(&s) {
        Ok(s)
    } else {
        Err(AppError::Validation(format!(
            "invalid note type: {s:?} (must be one of: {})",
            VALID_NOTE_TYPES.join(", ")
        )))
    }
}

/// Validate a note title. Trims surrounding whitespace, rejects
/// empty-after-trim and over-cap titles. Returns the trimmed slice
/// borrowed from `raw` on success.
pub(crate) fn validate_note_title(raw: &str) -> Result<&str, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(
            "note title must not be empty".to_string(),
        ));
    }
    if trimmed.chars().count() > MAX_NOTE_TITLE_LEN {
        return Err(AppError::Validation(format!(
            "note title exceeds {MAX_NOTE_TITLE_LEN} chars"
        )));
    }
    Ok(trimmed)
}

/// Columns selected from the notes table.
const SELECT_COLS: &str = "id, workspace_id, title, type, content, created_at, updated_at";

impl NoteStore {
    /// Insert a new note row. Fails with `Conflict` if
    /// `(workspace_id, title)` already exists (the UNIQUE index from
    /// migration 008); fails with `Validation` for invalid type or
    /// title; fails with `NotFound` if `workspace_id` does not exist
    /// (FK violation, mapped via `map_insert_error`).
    pub fn create(
        db: &Db,
        workspace_id: &str,
        title: &str,
        note_type: &str,
        content: &str,
    ) -> Result<Note, AppError> {
        let title = validate_note_title(title)?;
        let note_type = validate_note_type(note_type)?;
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        db.conn()
            .query_row(
                &format!(
                    "INSERT INTO notes ({SELECT_COLS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                     RETURNING {SELECT_COLS}"
                ),
                rusqlite::params![id, workspace_id, title, note_type, content, now],
                Self::map_row,
            )
            .map_err(|e| {
                crate::db::map_insert_error(
                    e,
                    "a note with the same title already exists in this workspace",
                    "workspace",
                )
            })
    }

    /// Fetch one note by id, workspace-scoped. Cross-workspace
    /// access returns `NotFound` (defense-in-depth — agents cannot
    /// enumerate notes across workspaces).
    pub fn get_by_id(db: &Db, note_id: &str, workspace_id: &str) -> Result<Note, AppError> {
        db.conn()
            .query_row(
                &format!("SELECT {SELECT_COLS} FROM notes WHERE id = ?1"),
                [note_id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "note".into(),
                    id: note_id.into(),
                },
                other => other.into(),
            })
            .and_then(|note| {
                if note.workspace_id != workspace_id {
                    Err(AppError::NotFound {
                        resource: "note".into(),
                        id: note_id.into(),
                    })
                } else {
                    Ok(note)
                }
            })
    }

    /// List a workspace's notes, optionally filtered by type. Ordered
    /// by `updated_at DESC, id DESC` so the most-recently-touched
    /// notes surface first (matching how `read_note`'s natural
    /// "what's new?" usage pattern benefits from a stable recency
    /// order). Capped at `DEFAULT_LIST_LIMIT` (500) to match the
    /// other list methods.
    pub fn list(
        db: &Db,
        workspace_id: &str,
        type_filter: Option<&str>,
    ) -> Result<Vec<Note>, AppError> {
        if let Some(t) = type_filter {
            validate_note_type(t)?;
        }
        let mut sql = format!("SELECT {SELECT_COLS} FROM notes WHERE workspace_id = ?1");
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(workspace_id.to_string())];
        if let Some(t) = type_filter {
            sql.push_str(" AND type = ?2");
            params.push(Box::new(t.to_string()));
        }
        sql.push_str(" ORDER BY updated_at DESC, id DESC LIMIT 500");

        let conn = db.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let rows: Vec<Note> = stmt
            .query_map(params_refs.as_slice(), Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Replace a note's content in-place. `id` and `created_at` are
    /// preserved; `updated_at` is bumped to `now`. Always performs
    /// the UPDATE (no no-op short-circuit) so a "touch" semantics
    /// (empty same-content) still bumps `updated_at` — matches the
    /// `artifact::provide` precedent. Cross-workspace access
    /// returns `NotFound`.
    pub fn set_content(
        db: &Db,
        note_id: &str,
        workspace_id: &str,
        content: &str,
    ) -> Result<Note, AppError> {
        // Scope check first: get_by_id returns NotFound for missing
        // OR cross-workspace, then we update by primary key.
        let _ = Self::get_by_id(db, note_id, workspace_id)?;
        let now = Utc::now().to_rfc3339();
        db.conn()
            .query_row(
                &format!(
                    "UPDATE notes SET content = ?2, updated_at = ?3
                     WHERE id = ?1
                     RETURNING {SELECT_COLS}"
                ),
                rusqlite::params![note_id, content, now],
                Self::map_row,
            )
            .map_err(AppError::from)
    }

    /// Append `suffix` to a note's content (single-statement atomic
    /// concatenation in SQL). No separator is inserted — the caller
    /// is responsible for any newline or punctuation between the
    /// existing content and the suffix. `updated_at` is bumped.
    /// Cross-workspace access returns `NotFound`.
    pub fn append(
        db: &Db,
        note_id: &str,
        workspace_id: &str,
        suffix: &str,
    ) -> Result<Note, AppError> {
        let _ = Self::get_by_id(db, note_id, workspace_id)?;
        let now = Utc::now().to_rfc3339();
        db.conn()
            .query_row(
                &format!(
                    "UPDATE notes SET content = content || ?2, updated_at = ?3
                     WHERE id = ?1
                     RETURNING {SELECT_COLS}"
                ),
                rusqlite::params![note_id, suffix, now],
                Self::map_row,
            )
            .map_err(AppError::from)
    }

    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
        Ok(Note {
            id: row.get(0)?,
            workspace_id: row.get(1)?,
            title: row.get(2)?,
            type_: row.get(3)?,
            content: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::make_test_db;

    /// Seed the default workspace. Returns its id.
    fn seed_workspace(db: &Db) -> String {
        crate::store::workspaces::WorkspaceStore::ensure_default(db).expect("ensure_default");
        db.conn()
            .query_row("SELECT id FROM workspaces WHERE name='default'", [], |r| {
                r.get(0)
            })
            .expect("default workspace")
    }

    /// Seed a second workspace alongside the default.
    fn seed_second_workspace(db: &Db) -> String {
        let ws2 = Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, ?2, 'active', ?3, ?3)",
                rusqlite::params![ws2, "ws2", Utc::now().to_rfc3339()],
            )
            .unwrap();
        ws2
    }

    // ---- validate_note_type ----

    #[test]
    fn test_validate_type_whitelist_accepts_all_three() {
        for t in VALID_NOTE_TYPES {
            assert!(validate_note_type(t).is_ok(), "{t} should be valid");
        }
    }

    #[test]
    fn test_validate_type_rejects_unknown_and_case_sensitive() {
        // Free-vocabulary rejected (artifact uses 'log', 'screenshot', etc.).
        assert!(validate_note_type("freeform").is_err());
        // Whitelist is case-sensitive: 'Spec' ≠ 'spec'.
        assert!(validate_note_type("Spec").is_err());
        // Trailing whitespace rejected.
        assert!(validate_note_type("spec ").is_err());
        // Empty rejected.
        assert!(validate_note_type("").is_err());
    }

    // ---- validate_note_title ----

    #[test]
    fn test_validate_title_rejects_empty() {
        assert!(validate_note_title("").is_err());
        assert!(validate_note_title("   ").is_err());
    }

    #[test]
    fn test_validate_title_rejects_too_long() {
        let over = "x".repeat(MAX_NOTE_TITLE_LEN + 1);
        let err = validate_note_title(&over).unwrap_err();
        match err {
            AppError::Validation(msg) => {
                assert!(msg.contains(&MAX_NOTE_TITLE_LEN.to_string()), "got: {msg}")
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
    }

    // ---- create ----

    #[test]
    fn test_create_persists_row() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let n = NoteStore::create(&db, &ws, "API contract", "spec", "first line").unwrap();
        assert_eq!(n.workspace_id, ws);
        assert_eq!(n.title, "API contract");
        assert_eq!(n.type_, "spec");
        assert_eq!(n.content, "first line");
        assert!(!n.id.is_empty());
        assert_eq!(n.created_at, n.updated_at);
    }

    #[test]
    fn test_create_trims_title_whitespace() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let n = NoteStore::create(&db, &ws, "  Padded  ", "general", "").unwrap();
        assert_eq!(
            n.title, "Padded",
            "leading/trailing whitespace should be trimmed"
        );
    }

    #[test]
    fn test_create_empty_content_is_allowed() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        // Spec-faithful: an agent creates a titled note, fills later.
        let n = NoteStore::create(&db, &ws, "stub", "general", "").unwrap();
        assert_eq!(n.content, "");
    }

    #[test]
    fn test_create_invalid_type_returns_validation() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let err = NoteStore::create(&db, &ws, "t", "freeform", "").unwrap_err();
        match err {
            AppError::Validation(msg) => {
                assert!(msg.contains("invalid note type"), "got: {msg}");
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
    }

    #[test]
    fn test_create_invalid_title_returns_validation() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let err = NoteStore::create(&db, &ws, "", "general", "").unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    #[test]
    fn test_create_duplicate_title_in_workspace_returns_conflict() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        NoteStore::create(&db, &ws, "t", "general", "").unwrap();
        let err = NoteStore::create(&db, &ws, "t", "spec", "").unwrap_err();
        match err {
            AppError::Conflict(msg) => {
                assert!(msg.contains("already exists"), "got: {msg}")
            }
            other => panic!("expected Conflict, got: {other:?}"),
        }
    }

    #[test]
    fn test_create_same_title_in_different_workspaces_is_allowed() {
        let db = make_test_db();
        let ws1 = seed_workspace(&db);
        let ws2 = seed_second_workspace(&db);
        NoteStore::create(&db, &ws1, "shared", "general", "").unwrap();
        NoteStore::create(&db, &ws2, "shared", "general", "").unwrap();
    }

    #[test]
    fn test_create_unknown_workspace_returns_not_found() {
        let db = make_test_db();
        let err = NoteStore::create(&db, "no-such-ws", "t", "general", "").unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "workspace"));
    }

    // ---- get_by_id ----

    #[test]
    fn test_get_by_id_returns_row() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let created = NoteStore::create(&db, &ws, "t", "general", "x").unwrap();
        let fetched = NoteStore::get_by_id(&db, &created.id, &ws).unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.content, "x");
    }

    #[test]
    fn test_get_by_id_unknown_returns_not_found() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let err = NoteStore::get_by_id(&db, "nope", &ws).unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "note"));
    }

    #[test]
    fn test_get_by_id_cross_workspace_returns_not_found() {
        let db = make_test_db();
        let ws1 = seed_workspace(&db);
        let created = NoteStore::create(&db, &ws1, "t", "general", "").unwrap();
        let err = NoteStore::get_by_id(&db, &created.id, "other-ws").unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "note"));
    }

    // ---- list ----

    #[test]
    fn test_list_with_type_filter() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        NoteStore::create(&db, &ws, "a", "spec", "").unwrap();
        NoteStore::create(&db, &ws, "b", "general", "").unwrap();
        NoteStore::create(&db, &ws, "c", "spec", "").unwrap();
        let specs = NoteStore::list(&db, &ws, Some("spec")).unwrap();
        assert_eq!(specs.len(), 2);
        assert!(specs.iter().all(|n| n.type_ == "spec"));
    }

    #[test]
    fn test_list_invalid_type_filter_returns_validation() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let err = NoteStore::list(&db, &ws, Some("freeform")).unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    #[test]
    fn test_list_excludes_other_workspaces() {
        let db = make_test_db();
        let ws1 = seed_workspace(&db);
        let ws2 = seed_second_workspace(&db);
        NoteStore::create(&db, &ws1, "in1", "general", "").unwrap();
        NoteStore::create(&db, &ws2, "in2", "general", "").unwrap();
        let l1 = NoteStore::list(&db, &ws1, None).unwrap();
        assert_eq!(l1.len(), 1);
        assert_eq!(l1[0].title, "in1");
    }

    #[test]
    fn test_list_ordered_by_updated_at_desc() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let a = NoteStore::create(&db, &ws, "a", "general", "").unwrap();
        // Sleep past rfc3339's second precision so the next create
        // has a strictly greater updated_at.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let b = NoteStore::create(&db, &ws, "b", "general", "").unwrap();
        // Touch `a` so it's the most-recent.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let _ = NoteStore::set_content(&db, &a.id, &ws, "touched").unwrap();
        let rows = NoteStore::list(&db, &ws, None).unwrap();
        assert_eq!(rows[0].id, a.id, "touched note must come first");
        assert_eq!(rows[1].id, b.id);
    }

    // ---- set_content ----

    #[test]
    fn test_set_content_replaces_and_bumps_updated_at() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let n = NoteStore::create(&db, &ws, "t", "general", "v1").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let updated = NoteStore::set_content(&db, &n.id, &ws, "v2").unwrap();
        assert_eq!(updated.content, "v2");
        assert_eq!(
            updated.created_at, n.created_at,
            "created_at must not change"
        );
        assert!(
            updated.updated_at > n.updated_at,
            "updated_at must bump ({} -> {})",
            n.updated_at,
            updated.updated_at
        );
    }

    #[test]
    fn test_set_content_cross_workspace_returns_not_found() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let n = NoteStore::create(&db, &ws, "t", "general", "").unwrap();
        let err = NoteStore::set_content(&db, &n.id, "other-ws", "x").unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "note"));
    }

    // ---- append ----

    #[test]
    fn test_append_grows_content_and_bumps_updated_at() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let n = NoteStore::create(&db, &ws, "t", "general", "v1").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let a1 = NoteStore::append(&db, &n.id, &ws, "-v2").unwrap();
        assert_eq!(a1.content, "v1-v2");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let a2 = NoteStore::append(&db, &n.id, &ws, "-v3").unwrap();
        assert_eq!(a2.content, "v1-v2-v3", "three appends must grow linearly");
        assert!(a2.updated_at > a1.updated_at);
    }

    #[test]
    fn test_append_cross_workspace_returns_not_found() {
        let db = make_test_db();
        let ws = seed_workspace(&db);
        let n = NoteStore::create(&db, &ws, "t", "general", "").unwrap();
        let err = NoteStore::append(&db, &n.id, "other-ws", "x").unwrap_err();
        assert!(matches!(err, AppError::NotFound { resource: r, .. } if r == "note"));
    }

    // ---- cascade ----

    #[test]
    fn test_workspace_deletion_cascades_notes() {
        let db = make_test_db();
        let ws2 = seed_second_workspace(&db);
        NoteStore::create(&db, &ws2, "a", "general", "").unwrap();
        NoteStore::create(&db, &ws2, "b", "spec", "").unwrap();
        db.conn()
            .execute("DELETE FROM workspaces WHERE id = ?1", [&ws2])
            .unwrap();
        let count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM notes WHERE workspace_id = ?1",
                [&ws2],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "ON DELETE CASCADE must remove notes");
    }
}
