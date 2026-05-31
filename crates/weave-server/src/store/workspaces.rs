use crate::db::Db;
use crate::error::AppError;
use chrono::Utc;
use rusqlite::ErrorCode;
use serde::Serialize;
use tracing::info;
use uuid::Uuid;

/// Domain representation of a workspace row.
#[derive(Debug, Clone, Serialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Cursor-based pagination result.
#[derive(Debug, Serialize)]
pub struct WorkspacePage {
    pub data: Vec<Workspace>,
    pub cursor: Option<String>,
}

/// Stateless store for workspace persistence.
///
/// All methods take `&Db` — no connection pooling, no lifetime management.
/// The caller holds the `MutexGuard` for the duration of each method call.
pub struct WorkspaceStore;

impl WorkspaceStore {
    /// Insert a new workspace. Returns the created row.
    ///
    /// Propagates UNIQUE constraint violations as `AppError::Validation`.
    pub fn create(db: &Db, name: &str) -> Result<Workspace, AppError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        db.conn()
            .query_row(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, ?2, 'active', ?3, ?4)
                 RETURNING id, name, status, created_at, updated_at",
                rusqlite::params![id, name, now, now],
                Self::map_row,
            )
            .map_err(Self::map_unique_violation)
    }

    /// Fetch a workspace by primary key.
    pub fn get_by_id(db: &Db, id: &str) -> Result<Workspace, AppError> {
        db.conn()
            .query_row(
                "SELECT id, name, status, created_at, updated_at
                 FROM workspaces WHERE id = ?1",
                [id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "workspace".into(),
                    id: id.into(),
                },
                other => other.into(),
            })
    }

    /// Cursor-based listing.
    ///
    /// Fetches up to `limit` rows after the cursor. If `limit` rows are
    /// returned, the last row's `id` becomes the next cursor.
    pub fn list(db: &Db, cursor: Option<&str>, limit: u32) -> Result<WorkspacePage, AppError> {
        let cursor = cursor.unwrap_or("");

        let conn = db.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, status, created_at, updated_at
             FROM workspaces
             WHERE id > ?1
             ORDER BY id ASC
             LIMIT ?2",
        )?;

        let rows: Vec<Workspace> = stmt
            .query_map(rusqlite::params![cursor, limit], Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;

        let next_cursor = if rows.len() == limit as usize {
            rows.last().map(|w| w.id.clone())
        } else {
            None
        };

        Ok(WorkspacePage {
            data: rows,
            cursor: next_cursor,
        })
    }

    /// Update a workspace's name. Returns the updated row.
    pub fn update_name(db: &Db, id: &str, new_name: &str) -> Result<Workspace, AppError> {
        let now = Utc::now().to_rfc3339();

        db.conn()
            .query_row(
                "UPDATE workspaces SET name = ?1, updated_at = ?2
                 WHERE id = ?3
                 RETURNING id, name, status, created_at, updated_at",
                rusqlite::params![new_name, now, id],
                Self::map_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                    resource: "workspace".into(),
                    id: id.into(),
                },
                other => Self::map_unique_violation(other),
            })
    }

    /// Hard delete a workspace. Cascades via FK constraints.
    ///
    /// `name` is passed in for logging — the caller already fetched it.
    pub fn delete(db: &Db, id: &str, name: &str) -> Result<(), AppError> {
        let rows_affected = db
            .conn()
            .execute("DELETE FROM workspaces WHERE id = ?1", [id])?;

        if rows_affected == 0 {
            return Err(AppError::NotFound {
                resource: "workspace".into(),
                id: id.into(),
            });
        }

        info!(workspace_id = %id, name = %name, "Workspace deleted");
        Ok(())
    }

    /// Seed the "default" workspace if it doesn't exist. Idempotent.
    pub fn ensure_default(db: &Db) -> Result<(), AppError> {
        let count: i32 = db.conn().query_row(
            "SELECT COUNT(*) FROM workspaces WHERE name = 'default'",
            [],
            |r| r.get(0),
        )?;

        if count == 0 {
            let id = Uuid::new_v4().to_string();
            let now = Utc::now().to_rfc3339();
            db.conn().execute(
                "INSERT OR IGNORE INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES (?1, 'default', 'active', ?2, ?2)",
                rusqlite::params![id, now],
            )?;
            info!("Default workspace created");
        }

        Ok(())
    }

    /// Map a result row to a `Workspace`.
    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
        Ok(Workspace {
            id: row.get(0)?,
            name: row.get(1)?,
            status: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    }

    /// Convert a UNIQUE constraint violation into an `AppError::Validation`.
    fn map_unique_violation(e: rusqlite::Error) -> AppError {
        if let rusqlite::Error::SqliteFailure(ref err, _) = e {
            if err.code == ErrorCode::ConstraintViolation {
                // SQLITE_CONSTRAINT_UNIQUE = 2067
                if err.extended_code == 2067 {
                    return AppError::Validation("workspace name already exists".into());
                }
            }
        }
        e.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_db() -> Db {
        Db::open(Path::new(":memory:")).expect("failed to open test db")
    }

    #[test]
    fn test_create_workspace() {
        let db = test_db();
        let ws = WorkspaceStore::create(&db, "my-project").unwrap();

        assert!(!ws.id.is_empty());
        assert_eq!(ws.name, "my-project");
        assert_eq!(ws.status, "active");
        assert!(!ws.created_at.is_empty());
        assert!(!ws.updated_at.is_empty());
    }

    #[test]
    fn test_create_duplicate_name() {
        let db = test_db();
        WorkspaceStore::create(&db, "unique-name").unwrap();
        let result = WorkspaceStore::create(&db, "unique-name");

        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::Validation(msg) => {
                assert!(msg.contains("already exists"), "got: {}", msg);
            }
            other => panic!("expected Validation, got: {:?}", other),
        }
    }

    #[test]
    fn test_get_by_id() {
        let db = test_db();
        let created = WorkspaceStore::create(&db, "fetch-me").unwrap();
        let fetched = WorkspaceStore::get_by_id(&db, &created.id).unwrap();

        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, "fetch-me");
    }

    #[test]
    fn test_get_by_id_not_found() {
        let db = test_db();
        let result = WorkspaceStore::get_by_id(&db, "nonexistent");

        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { resource, id } => {
                assert_eq!(resource, "workspace");
                assert_eq!(id, "nonexistent");
            }
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_list_pagination() {
        let db = test_db();
        for i in 0..5 {
            WorkspaceStore::create(&db, &format!("ws-{}", i)).unwrap();
        }

        // Verify total count
        let total: i32 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM workspaces", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 5, "should have 5 workspaces");

        // First page: limit 2, fetches 3 (limit+1), pops last → 2 items + cursor
        let page1 = WorkspaceStore::list(&db, None, 2).unwrap();
        assert_eq!(page1.data.len(), 2, "first page should have 2 items");
        assert!(page1.cursor.is_some(), "should have next cursor");

        // Second page: fetches remaining items
        let page2 = WorkspaceStore::list(&db, page1.cursor.as_deref(), 2).unwrap();
        assert!(!page2.data.is_empty(), "second page should have items");

        // Collect all items across pages
        let mut all_ids: Vec<String> = page1.data.iter().map(|w| w.id.clone()).collect();
        all_ids.extend(page2.data.iter().map(|w| w.id.clone()));

        // If there are more pages, keep fetching
        let mut cursor = page2.cursor.clone();
        while let Some(c) = cursor {
            let page = WorkspaceStore::list(&db, Some(&c), 2).unwrap();
            all_ids.extend(page.data.iter().map(|w| w.id.clone()));
            cursor = page.cursor;
        }

        assert_eq!(all_ids.len(), 5, "should paginate through all 5 workspaces");

        // Verify all IDs are unique (no duplicates across pages)
        let unique: std::collections::HashSet<_> = all_ids.iter().collect();
        assert_eq!(unique.len(), 5, "all IDs should be unique");
    }

    #[test]
    fn test_list_empty() {
        let db = test_db();
        let page = WorkspaceStore::list(&db, None, 10).unwrap();

        assert!(page.data.is_empty());
        assert!(page.cursor.is_none());
    }

    #[test]
    fn test_update_name() {
        let db = test_db();
        let ws = WorkspaceStore::create(&db, "old-name").unwrap();
        let updated = WorkspaceStore::update_name(&db, &ws.id, "new-name").unwrap();

        assert_eq!(updated.name, "new-name");
        assert_eq!(updated.id, ws.id);
    }

    #[test]
    fn test_update_not_found() {
        let db = test_db();
        let result = WorkspaceStore::update_name(&db, "nonexistent", "new-name");

        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { resource, .. } => assert_eq!(resource, "workspace"),
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_delete() {
        let db = test_db();
        let ws = WorkspaceStore::create(&db, "to-delete").unwrap();
        WorkspaceStore::delete(&db, &ws.id, &ws.name).unwrap();

        let result = WorkspaceStore::get_by_id(&db, &ws.id);
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_not_found() {
        let db = test_db();
        let result = WorkspaceStore::delete(&db, "nonexistent", "unknown");

        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::NotFound { .. } => {}
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_ensure_default_idempotent() {
        let db = test_db();
        WorkspaceStore::ensure_default(&db).unwrap();
        WorkspaceStore::ensure_default(&db).unwrap(); // second call is no-op

        // Verify exactly one "default" workspace exists
        let count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM workspaces WHERE name = 'default'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "should have exactly one default workspace");
    }
}
