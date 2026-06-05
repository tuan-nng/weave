use crate::db::Db;
use crate::error::AppError;
use chrono::Utc;
use rusqlite::Connection;
use rusqlite::ErrorCode;
use serde::Serialize;
use std::collections::BTreeMap;
use tracing::info;
use uuid::Uuid;

/// Maximum depth for parent session chain resolution.
pub(crate) const MAX_RESUME_DEPTH: usize = 5;

// ---------------------------------------------------------------------------
// Session types
// ---------------------------------------------------------------------------

/// Domain representation of a session row.
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub workspace_id: String,
    pub provider_id: String,
    pub specialist_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub context_id: Option<String>,
    pub status: String,
    pub model: Option<String>,
    pub cwd: Option<String>,
    /// Set when the session was created with `codebase_id` in the
    /// create request. When the referenced codebase is deleted, the
    /// FK is `ON DELETE SET NULL` — the row survives, the binding
    /// is broken, the stored `cwd` is kept as-is.
    pub codebase_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Cursor-based pagination result for sessions.
#[derive(Debug, Serialize)]
pub struct SessionPage {
    pub data: Vec<Session>,
    pub cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

/// Domain representation of a message row (immutable).
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub metadata: Option<String>,
    pub created_at: String,
}

/// Cursor-based pagination result for messages.
#[derive(Debug, Serialize)]
pub struct MessagePage {
    pub data: Vec<Message>,
    pub cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

/// All valid session statuses.
const VALID_STATUSES: &[&str] = &["connecting", "ready", "completed", "cancelled", "error"];

/// Terminal states — no transitions out.
/// Used in SQL WHERE clause and by SessionService for validation.
pub(crate) const TERMINAL: &[&str] = &["completed", "cancelled", "error"];

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Convert a foreign key violation into a clear `AppError::Validation`.
///
/// Catches SQLITE_CONSTRAINT_FOREIGNKEY (extended code 787) for
/// provider_id and parent_session_id references.
fn map_fk_violation(e: rusqlite::Error) -> AppError {
    if let rusqlite::Error::SqliteFailure(ref err, _) = e {
        if err.code == ErrorCode::ConstraintViolation {
            // SQLITE_CONSTRAINT_FOREIGNKEY = 787
            if err.extended_code == 787 {
                return AppError::Validation(
                    "referenced resource not found (workspace_id, provider_id, or parent_session_id)"
                        .into(),
                );
            }
        }
    }
    e.into()
}

// ---------------------------------------------------------------------------
// SessionStore
// ---------------------------------------------------------------------------

/// Stateless store for session persistence.
///
/// All methods take `&Db` — no connection pooling, no lifetime management.
pub struct SessionStore;

impl SessionStore {
    /// Insert a new session. Returns the created row.
    ///
    /// Validates foreign keys: provider_id must reference an existing provider.
    /// parent_session_id (if set) must reference an existing session.
    /// codebase_id (if set) must reference an existing codebase; the FK
    /// column has `ON DELETE SET NULL`, so a deleted codebase unbinds
    /// the session rather than removing it.
    ///
    /// Prefer `SessionService::create_session` for production code (supports resume).
    #[allow(dead_code)] // Used in tests; production code uses SessionService::create_session
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        db: &Db,
        workspace_id: &str,
        provider_id: &str,
        specialist_id: Option<&str>,
        model: Option<&str>,
        cwd: Option<&str>,
        parent_session_id: Option<&str>,
        context_id: Option<&str>,
        codebase_id: Option<&str>,
    ) -> Result<Session, AppError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        db.conn()
            .query_row(
                "INSERT INTO sessions
                     (id, workspace_id, provider_id, specialist_id,
                      parent_session_id, context_id, status, model, cwd, codebase_id,
                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'connecting', ?7, ?8, ?9, ?10, ?10)
                 RETURNING id, workspace_id, provider_id, specialist_id,
                           parent_session_id, context_id, status, model, cwd, codebase_id,
                           created_at, updated_at",
                rusqlite::params![
                    id,
                    workspace_id,
                    provider_id,
                    specialist_id,
                    parent_session_id,
                    context_id,
                    model,
                    cwd,
                    codebase_id,
                    now,
                ],
                Self::map_row,
            )
            .map_err(map_fk_violation)
    }

    /// Fetch a session by primary key.
    pub fn get_by_id(db: &Db, id: &str) -> Result<Session, AppError> {
        db.conn()
            .query_row(
                "SELECT id, workspace_id, provider_id, specialist_id,
                        parent_session_id, context_id, status, model, cwd, codebase_id,
                        created_at, updated_at
                 FROM sessions WHERE id = ?1",
                [id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "session".into(),
                    id: id.into(),
                },
                other => other.into(),
            })
    }

    /// Cursor-based listing by workspace.
    ///
    /// Fetches up to `limit` rows after the cursor, ordered by `id`.
    pub fn list_by_workspace(
        db: &Db,
        workspace_id: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<SessionPage, AppError> {
        let cursor = cursor.unwrap_or("");

        let conn = db.conn();
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, provider_id, specialist_id,
                    parent_session_id, context_id, status, model, cwd, codebase_id,
                    created_at, updated_at
             FROM sessions
             WHERE workspace_id = ?1 AND id > ?2
             ORDER BY id ASC
             LIMIT ?3",
        )?;

        let rows: Vec<Session> = stmt
            .query_map(
                rusqlite::params![workspace_id, cursor, limit],
                Self::map_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;

        let next_cursor = if rows.len() == limit as usize {
            rows.last().map(|s| s.id.clone())
        } else {
            None
        };

        Ok(SessionPage {
            data: rows,
            cursor: next_cursor,
        })
    }

    /// Update a session's status. Enforces the state machine atomically.
    ///
    /// Validates `new_status` against known statuses. The SQL WHERE clause
    /// rejects transitions from terminal states, preventing TOCTOU races.
    pub fn update_status(db: &Db, id: &str, new_status: &str) -> Result<Session, AppError> {
        if !VALID_STATUSES.contains(&new_status) {
            return Err(AppError::Validation(format!(
                "invalid status '{}', expected one of: {:?}",
                new_status, VALID_STATUSES,
            )));
        }

        let now = Utc::now().to_rfc3339();

        let result = db.conn().query_row(
            "UPDATE sessions SET status = ?1, updated_at = ?2
             WHERE id = ?3 AND status NOT IN ('completed', 'cancelled', 'error')
             RETURNING id, workspace_id, provider_id, specialist_id,
                       parent_session_id, context_id, status, model, cwd, codebase_id,
                       created_at, updated_at",
            rusqlite::params![new_status, now, id],
            Self::map_row,
        );

        match result {
            Ok(session) => Ok(session),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                // Disambiguate: not found vs illegal transition
                let current = Self::get_by_id(db, id)?;
                Err(AppError::Validation(format!(
                    "cannot transition from '{}' to '{}'",
                    current.status, new_status,
                )))
            }
            Err(other) => Err(other.into()),
        }
    }

    /// Hard delete a session. Messages cascade via FK constraints.
    pub fn delete(db: &Db, id: &str) -> Result<(), AppError> {
        let rows_affected = db
            .conn()
            .execute("DELETE FROM sessions WHERE id = ?1", [id])?;

        if rows_affected == 0 {
            return Err(AppError::NotFound {
                resource: "session".into(),
                id: id.into(),
            });
        }

        info!(session_id = %id, "Session deleted");
        Ok(())
    }

    /// Map a result row to a `Session`.
    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
        Ok(Session {
            id: row.get(0)?,
            workspace_id: row.get(1)?,
            provider_id: row.get(2)?,
            specialist_id: row.get(3)?,
            parent_session_id: row.get(4)?,
            context_id: row.get(5)?,
            status: row.get(6)?,
            model: row.get(7)?,
            cwd: row.get(8)?,
            codebase_id: row.get(9)?,
            created_at: row.get(10)?,
            updated_at: row.get(11)?,
        })
    }

    /// Insert a new session within an existing transaction.
    ///
    /// Same as `create` but takes `&Connection` for use inside `Db::with_transaction`.
    /// `codebase_id` is bound as `?9`; the FK has `ON DELETE SET NULL`, so a
    /// deleted codebase unbinds the session rather than removing it.
    #[allow(clippy::too_many_arguments)]
    pub fn create_tx(
        conn: &Connection,
        workspace_id: &str,
        provider_id: &str,
        specialist_id: Option<&str>,
        model: Option<&str>,
        cwd: Option<&str>,
        parent_session_id: Option<&str>,
        context_id: Option<&str>,
        codebase_id: Option<&str>,
    ) -> Result<Session, AppError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        conn.query_row(
            "INSERT INTO sessions
                 (id, workspace_id, provider_id, specialist_id,
                  parent_session_id, context_id, status, model, cwd, codebase_id,
                  created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'connecting', ?7, ?8, ?9, ?10, ?10)
             RETURNING id, workspace_id, provider_id, specialist_id,
                       parent_session_id, context_id, status, model, cwd, codebase_id,
                       created_at, updated_at",
            rusqlite::params![
                id,
                workspace_id,
                provider_id,
                specialist_id,
                parent_session_id,
                context_id,
                model,
                cwd,
                codebase_id,
                now,
            ],
            Self::map_row,
        )
        .map_err(map_fk_violation)
    }

    /// Count non-terminal sessions grouped by workspace.
    ///
    /// A session is "active" iff its `status` is not in [`TERMINAL`]
    /// (i.e. not `completed`, `cancelled`, or `error`). The returned
    /// `BTreeMap` is sorted by `workspace_id` (deterministic JSON key
    /// order). Workspaces with zero active sessions are absent from
    /// the result (`GROUP BY` only emits non-empty groups).
    ///
    /// Used by `GET /api/health` to report per-workspace active counts
    /// without enumerating individual sessions. Reads are served by the
    /// existing `idx_sessions_workspace` and `idx_sessions_status`.
    pub fn count_active_by_workspace(db: &Db) -> Result<BTreeMap<String, u64>, AppError> {
        // Build the `IN (?, ?, ?)` placeholder list at runtime so the
        // status names stay in sync with the [`TERMINAL`] const.
        let placeholders = TERMINAL.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT workspace_id, COUNT(*) FROM sessions
             WHERE status NOT IN ({placeholders})
             GROUP BY workspace_id"
        );

        let conn = db.conn();
        let mut stmt = conn.prepare(&sql)?;
        let mut out = BTreeMap::new();
        let mut rows = stmt.query(rusqlite::params_from_iter(TERMINAL.iter().copied()))?;
        while let Some(row) = rows.next()? {
            let ws: String = row.get(0)?;
            let n: i64 = row.get(1)?;
            out.insert(ws, n as u64);
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// MessageStore
// ---------------------------------------------------------------------------

/// Stateless store for message persistence.
///
/// Messages are immutable — no update or status transition.
pub struct MessageStore;

impl MessageStore {
    /// Insert a new message. Returns the created row.
    pub fn create(
        db: &Db,
        session_id: &str,
        role: &str,
        content: &str,
        metadata: Option<&str>,
    ) -> Result<Message, AppError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        db.conn()
            .query_row(
                "INSERT INTO messages (id, session_id, role, content, metadata, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 RETURNING id, session_id, role, content, metadata, created_at",
                rusqlite::params![id, session_id, role, content, metadata, now],
                Self::map_row,
            )
            .map_err(AppError::from)
    }

    /// Cursor-based listing by session, ordered by `created_at` (asc).
    ///
    /// Messages are sorted by their `created_at` timestamp with `id` as a
    /// deterministic tiebreaker for sub-second ties. The cursor is a
    /// `"<created_at>|<id>"` pair encoding the last message of the previous
    /// page; on the next call, only messages strictly after that pair are
    /// returned.
    ///
    /// Sorting by `created_at` (not `id`) is required because message IDs
    /// are random v4 UUIDs and carry no time order — sorting by `id` produced
    /// a random message order in the UI.
    pub fn list_by_session(
        db: &Db,
        session_id: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<MessagePage, AppError> {
        // Parse the cursor into (created_at, id). Empty / None means "start".
        let (cursor_ts, cursor_id) = match cursor {
            Some(c) if !c.is_empty() => match c.split_once('|') {
                Some((ts, id)) => (ts.to_string(), id.to_string()),
                None => {
                    return Err(AppError::Validation(format!(
                        "invalid cursor: expected '<created_at>|<id>', got {c:?}"
                    )));
                }
            },
            _ => (String::new(), String::new()),
        };

        let conn = db.conn();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, role, content, metadata, created_at
             FROM messages
             WHERE session_id = ?1
               AND (created_at > ?2 OR (created_at = ?2 AND id > ?3))
             ORDER BY created_at ASC, id ASC
             LIMIT ?4",
        )?;

        let rows: Vec<Message> = stmt
            .query_map(
                rusqlite::params![session_id, cursor_ts, cursor_id, limit],
                Self::map_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;

        let next_cursor = if rows.len() == limit as usize {
            rows.last().map(|m| format!("{}|{}", m.created_at, m.id))
        } else {
            None
        };

        Ok(MessagePage {
            data: rows,
            cursor: next_cursor,
        })
    }

    /// Map a result row to a `Message`.
    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
        Ok(Message {
            id: row.get(0)?,
            session_id: row.get(1)?,
            role: row.get(2)?,
            content: row.get(3)?,
            metadata: row.get(4)?,
            created_at: row.get(5)?,
        })
    }

    /// Bulk-copy messages into a target session within an existing transaction.
    ///
    /// Each message gets a new UUID but preserves the original `role`, `content`,
    /// `metadata`, and `created_at`. Returns the number of messages copied.
    pub fn copy_messages(
        conn: &Connection,
        target_session_id: &str,
        messages: &[Message],
    ) -> Result<u64, AppError> {
        if messages.is_empty() {
            return Ok(0);
        }

        let mut stmt = conn.prepare(
            "INSERT INTO messages (id, session_id, role, content, metadata, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        let mut count = 0u64;
        for msg in messages {
            let new_id = Uuid::new_v4().to_string();
            stmt.execute(rusqlite::params![
                new_id,
                target_session_id,
                msg.role,
                msg.content,
                msg.metadata,
                msg.created_at,
            ])?;
            count += 1;
        }

        Ok(count)
    }

    /// Load all messages for a session (paginated, up to `max`).
    ///
    /// Reuses cursor-based pagination internally.
    pub fn load_all(db: &Db, session_id: &str, max: usize) -> Result<Vec<Message>, AppError> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let page = Self::list_by_session(db, session_id, cursor.as_deref(), 100)?;
            let has_more = page.cursor.is_some();
            cursor = page.cursor;
            all.extend(page.data);
            if !has_more || all.len() >= max {
                break;
            }
        }

        all.truncate(max);
        Ok(all)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::Path;

    fn test_db() -> Db {
        Db::open(Path::new(":memory:")).expect("failed to open test db")
    }

    /// Seed a workspace and provider, return (workspace_id, provider_id).
    pub(crate) fn seed_deps(db: &Db) -> (String, String) {
        let ws = crate::store::workspaces::WorkspaceStore::create(db, "test-ws").unwrap();
        let config = serde_json::json!({
            "base_url": "https://api.anthropic.com",
            "api_key": "sk-test",
            "default_model": "claude-sonnet-4-20250514"
        })
        .to_string();
        let provider =
            crate::store::providers::ProviderStore::create(db, "anthropic", "Test", &config)
                .unwrap();
        (ws.id, provider.id)
    }

    // --- Session tests ---

    #[test]
    fn test_session_lifecycle() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Create
        let session = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            Some("claude-sonnet-4-20250514"),
            Some("/tmp"),
            None,
            None,
            None, // codebase_id
        )
        .unwrap();

        assert!(!session.id.is_empty());
        assert_eq!(session.workspace_id, ws_id);
        assert_eq!(session.provider_id, provider_id);
        assert_eq!(session.status, "connecting");
        assert_eq!(session.model.as_deref(), Some("claude-sonnet-4-20250514"));
        assert_eq!(session.cwd.as_deref(), Some("/tmp"));
        assert!(session.specialist_id.is_none());
        assert!(session.parent_session_id.is_none());

        // Get by ID
        let fetched = SessionStore::get_by_id(&db, &session.id).unwrap();
        assert_eq!(fetched.id, session.id);

        // Update status
        let ready = SessionStore::update_status(&db, &session.id, "ready").unwrap();
        assert_eq!(ready.status, "ready");

        // Delete
        SessionStore::delete(&db, &session.id).unwrap();
        let result = SessionStore::get_by_id(&db, &session.id);
        assert!(result.is_err());
    }

    #[test]
    fn test_session_state_transitions() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Valid transitions from 'connecting'
        let s = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(s.status, "connecting");

        let s = SessionStore::update_status(&db, &s.id, "ready").unwrap();
        assert_eq!(s.status, "ready");

        let s = SessionStore::update_status(&db, &s.id, "completed").unwrap();
        assert_eq!(s.status, "completed");

        // Terminal state — no transitions out
        let result = SessionStore::update_status(&db, &s.id, "ready");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::Validation(msg) => {
                assert!(msg.contains("cannot transition"), "got: {}", msg);
            }
            other => panic!("expected Validation, got: {:?}", other),
        }
    }

    #[test]
    fn test_session_terminal_states() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // cancelled is terminal
        let s = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let s = SessionStore::update_status(&db, &s.id, "cancelled").unwrap();
        assert!(SessionStore::update_status(&db, &s.id, "ready").is_err());

        // error is terminal
        let s = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let s = SessionStore::update_status(&db, &s.id, "error").unwrap();
        assert!(SessionStore::update_status(&db, &s.id, "ready").is_err());
    }

    #[test]
    fn test_session_list_pagination() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        for _ in 0..5 {
            SessionStore::create(
                &db,
                &ws_id,
                &provider_id,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        }

        let page1 = SessionStore::list_by_workspace(&db, &ws_id, None, 2).unwrap();
        assert_eq!(page1.data.len(), 2);
        assert!(page1.cursor.is_some());

        let page2 =
            SessionStore::list_by_workspace(&db, &ws_id, page1.cursor.as_deref(), 2).unwrap();
        assert_eq!(page2.data.len(), 2);

        let page3 =
            SessionStore::list_by_workspace(&db, &ws_id, page2.cursor.as_deref(), 2).unwrap();
        assert_eq!(page3.data.len(), 1);
        assert!(page3.cursor.is_none());
    }

    #[test]
    fn test_session_list_empty() {
        let db = test_db();
        let (ws_id, _) = seed_deps(&db);

        let page = SessionStore::list_by_workspace(&db, &ws_id, None, 10).unwrap();
        assert!(page.data.is_empty());
        assert!(page.cursor.is_none());
    }

    #[test]
    fn test_session_get_not_found() {
        let db = test_db();
        let result = SessionStore::get_by_id(&db, "nonexistent");

        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { resource, id } => {
                assert_eq!(resource, "session");
                assert_eq!(id, "nonexistent");
            }
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_session_delete_not_found() {
        let db = test_db();
        let result = SessionStore::delete(&db, "nonexistent");

        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { .. } => {}
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_session_fk_violation_invalid_provider() {
        let db = test_db();
        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();

        let result = SessionStore::create(
            &db,
            &ws.id,
            "nonexistent-provider",
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
        );
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::Validation(msg) => {
                assert!(msg.contains("referenced resource"), "got: {}", msg);
            }
            other => panic!("expected Validation, got: {:?}", other),
        }
    }

    #[test]
    fn test_session_with_parent() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        let parent = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let child = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            Some(&parent.id),
            None,
            None, // codebase_id
        )
        .unwrap();

        assert_eq!(child.parent_session_id.as_deref(), Some(parent.id.as_str()));
    }

    /// Session with `codebase_id` round-trips through INSERT/SELECT,
    /// preserving the binding alongside `cwd`.
    #[test]
    fn test_session_with_codebase_id() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Register a real (git-free is fine for this test) codebase.
        let tmp = tempfile::TempDir::new().unwrap();
        let codebase = crate::store::codebases::CodebaseStore::create(
            &db,
            &ws_id,
            tmp.path().to_str().unwrap(),
            None,
            Some("test-codebase"),
        )
        .unwrap();

        let session = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            Some(tmp.path().to_str().unwrap()),
            None,
            None,
            Some(&codebase.id),
        )
        .unwrap();

        assert_eq!(session.codebase_id.as_deref(), Some(codebase.id.as_str()));
        assert_eq!(session.cwd.as_deref(), Some(tmp.path().to_str().unwrap()));

        // Round-trip via get_by_id
        let fetched = SessionStore::get_by_id(&db, &session.id).unwrap();
        assert_eq!(fetched.codebase_id.as_deref(), Some(codebase.id.as_str()));

        // Round-trip via list_by_workspace
        let page = SessionStore::list_by_workspace(&db, &ws_id, None, 50).unwrap();
        assert_eq!(page.data.len(), 1);
        assert_eq!(
            page.data[0].codebase_id.as_deref(),
            Some(codebase.id.as_str())
        );
    }

    /// ON DELETE SET NULL: deleting a codebase unbinds the session
    /// but does not delete the session row. The `cwd` we copied in
    /// remains — the session just becomes detached.
    #[test]
    fn test_session_codebase_delete_sets_null() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        let tmp = tempfile::TempDir::new().unwrap();
        let codebase = crate::store::codebases::CodebaseStore::create(
            &db,
            &ws_id,
            tmp.path().to_str().unwrap(),
            None,
            Some("to-delete"),
        )
        .unwrap();

        let session = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            Some(tmp.path().to_str().unwrap()),
            None,
            None,
            Some(&codebase.id),
        )
        .unwrap();
        assert!(session.codebase_id.is_some());

        // Delete the codebase
        crate::store::codebases::CodebaseStore::delete(&db, &codebase.id).unwrap();

        // Session survives, binding is gone, cwd remains
        let fetched = SessionStore::get_by_id(&db, &session.id).unwrap();
        assert!(fetched.codebase_id.is_none());
        assert_eq!(fetched.cwd.as_deref(), Some(tmp.path().to_str().unwrap()));
    }

    // --- Message tests ---

    #[test]
    fn test_message_pagination() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Create 5 messages
        for i in 0..5 {
            MessageStore::create(
                &db,
                &session.id,
                "user",
                &format!(r#"{{"text":"message {}"}}"#, i),
                None,
            )
            .unwrap();
        }

        // Paginate with limit 2
        let page1 = MessageStore::list_by_session(&db, &session.id, None, 2).unwrap();
        assert_eq!(page1.data.len(), 2);
        assert!(page1.cursor.is_some());

        let page2 =
            MessageStore::list_by_session(&db, &session.id, page1.cursor.as_deref(), 2).unwrap();
        assert_eq!(page2.data.len(), 2);

        let page3 =
            MessageStore::list_by_session(&db, &session.id, page2.cursor.as_deref(), 2).unwrap();
        assert_eq!(page3.data.len(), 1);
        assert!(page3.cursor.is_none());

        // Verify created_at ordering (cursor-based pagination uses
        // created_at + id; created_at is monotonically non-decreasing for
        // messages inserted in order, so the tiebreaker (id) is what
        // disambiguates sub-second ties).
        let all: Vec<&Message> = page1
            .data
            .iter()
            .chain(page2.data.iter())
            .chain(page3.data.iter())
            .collect();
        assert_eq!(all.len(), 5);
        for window in all.windows(2) {
            assert!(
                window[0].created_at < window[1].created_at
                    || (window[0].created_at == window[1].created_at
                        && window[0].id <= window[1].id),
                "messages out of order: {:?} then {:?}",
                window[0],
                window[1]
            );
        }
    }

    /// Regression: list_by_session must order by created_at, not by id.
    ///
    /// IDs are random v4 UUIDs and carry no time order. Previously, sorting
    /// by `id` produced a random message order in the UI (e.g. assistant
    /// messages appearing above their user prompts).
    #[test]
    fn test_message_list_orders_by_created_at_not_id() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Create 4 messages in order. We use the `id` field of the returned
        // Message to assert that the *insertion* order is preserved by the
        // listing, regardless of UUID randomness.
        let mut created_ids = Vec::new();
        for i in 0..4 {
            let m = MessageStore::create(
                &db,
                &session.id,
                "user",
                &format!(r#"{{"text":"m{}"}}"#, i),
                None,
            )
            .unwrap();
            created_ids.push(m.id);
        }

        let page = MessageStore::list_by_session(&db, &session.id, None, 10).unwrap();
        let listed_ids: Vec<&str> = page.data.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            listed_ids, created_ids,
            "list_by_session must return messages in insertion order"
        );
    }

    #[test]
    fn test_message_create_and_fetch() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let msg = MessageStore::create(
            &db,
            &session.id,
            "assistant",
            r#"{"text":"hello"}"#,
            Some(r#"{"model":"claude-sonnet-4-20250514"}"#),
        )
        .unwrap();

        assert!(!msg.id.is_empty());
        assert_eq!(msg.session_id, session.id);
        assert_eq!(msg.role, "assistant");
        assert_eq!(msg.content, r#"{"text":"hello"}"#);
        assert_eq!(
            msg.metadata.as_deref(),
            Some(r#"{"model":"claude-sonnet-4-20250514"}"#)
        );
    }

    #[test]
    fn test_message_list_empty() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let page = MessageStore::list_by_session(&db, &session.id, None, 10).unwrap();
        assert!(page.data.is_empty());
        assert!(page.cursor.is_none());
    }

    #[test]
    fn test_session_delete_cascades_messages() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        MessageStore::create(&db, &session.id, "user", r#"{"text":"hi"}"#, None).unwrap();
        MessageStore::create(&db, &session.id, "assistant", r#"{"text":"hello"}"#, None).unwrap();

        // Delete session — messages should cascade
        SessionStore::delete(&db, &session.id).unwrap();

        let page = MessageStore::list_by_session(&db, &session.id, None, 10).unwrap();
        assert!(
            page.data.is_empty(),
            "messages should be cascade-deleted with session"
        );
    }

    /// `count_active_by_workspace` excludes terminal sessions, omits
    /// workspaces with zero active sessions, and returns a `BTreeMap`
    /// sorted by `workspace_id`.
    #[test]
    fn test_count_active_by_workspace_excludes_terminal() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);
        // Need a second workspace — create one directly.
        let ws2_id = crate::store::workspaces::WorkspaceStore::create(&db, "ws2")
            .expect("create ws2")
            .id;

        // ws1: 2 active, 1 terminal
        let s1a = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let s1b = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let s1t = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        SessionStore::update_status(&db, &s1a.id, "ready").unwrap();
        SessionStore::update_status(&db, &s1b.id, "ready").unwrap();
        SessionStore::update_status(&db, &s1t.id, "completed").unwrap();

        // ws2: 1 active
        let s2a = SessionStore::create(
            &db,
            &ws2_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        SessionStore::update_status(&db, &s2a.id, "ready").unwrap();

        let map = SessionStore::count_active_by_workspace(&db).expect("count");

        assert_eq!(
            map.get(&ws_id),
            Some(&2),
            "ws1: 2 active, 1 terminal excluded"
        );
        assert_eq!(map.get(&ws2_id), Some(&1), "ws2: 1 active");
        assert_eq!(map.len(), 2, "only two workspaces with active sessions");
    }
}
