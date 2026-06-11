use crate::db::Db;
use crate::error::AppError;
use serde::Serialize;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Event types (flow through the mpsc channel)
// ---------------------------------------------------------------------------

/// A trace event emitted during agent execution.
///
/// Flows from the session streaming loop through an mpsc channel
/// to the background flush task, which batch-inserts into SQLite.
#[derive(Debug, Clone)]
pub struct TraceEvent {
    pub session_id: String,
    pub kind: TraceEventKind,
    pub timestamp: String,
}

/// Discriminated trace event payloads.
#[derive(Debug, Clone)]
pub enum TraceEventKind {
    ToolCall {
        tool_name: String,
        input_json: String,
        output_json: String,
        duration_ms: u64,
        /// `None` for normal completed tool calls. `Some("orphaned")` for
        /// CLI tool_use blocks that the CLI never paired with a
        /// `tool_result` before the turn ended (feat-048). Additive — older
        /// readers tolerate the field being absent in stored `data_json`
        /// payloads.
        status: Option<String>,
    },
    FileChange {
        path: String,
        action: FileAction,
    },
    Decision {
        text: String,
    },
    Error {
        message: String,
    },
}

/// File change action types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAction {
    Read,
    Write,
    Create,
    Delete,
}

impl FileAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Create => "create",
            Self::Delete => "delete",
        }
    }
}

// ---------------------------------------------------------------------------
// DB row types
// ---------------------------------------------------------------------------

/// Domain representation of a trace row.
#[derive(Debug, Clone, Serialize)]
pub struct TraceRow {
    pub id: String,
    pub session_id: String,
    pub event_type: String,
    pub summary: String,
    pub data_json: Option<String>,
    pub timestamp: String,
}

/// Deduplicated file change summary for the API response.
#[derive(Debug, Clone, Serialize)]
pub struct FileChangeSummary {
    pub path: String,
    pub actions: Vec<String>,
    pub count: u32,
}

// ---------------------------------------------------------------------------
// TraceStore
// ---------------------------------------------------------------------------

const TRACE_COLS: &str = "id, session_id, event_type, summary, data_json, timestamp";

/// Stateless store for trace persistence.
///
/// All methods take `&Db`. The flush task calls `insert_batch` to write
/// events; API handlers call `list_*` methods to read them.
pub struct TraceStore;

impl TraceStore {
    /// Batch-insert trace events and their associated file_changes in a
    /// single transaction. Called by the background flush task.
    pub fn insert_batch(db: &Db, events: &[TraceEvent]) -> Result<(), AppError> {
        let conn = db.conn();
        conn.execute_batch("BEGIN")?;

        let result = Self::insert_batch_inner(&conn, events);

        if result.is_ok() {
            conn.execute_batch("COMMIT")?;
        } else {
            let _ = conn.execute_batch("ROLLBACK");
        }

        result
    }

    fn insert_batch_inner(
        conn: &rusqlite::Connection,
        events: &[TraceEvent],
    ) -> Result<(), AppError> {
        for event in events {
            let trace_id = Uuid::new_v4().to_string();
            let (event_type, summary, data_json) = match &event.kind {
                TraceEventKind::ToolCall {
                    tool_name,
                    input_json,
                    output_json,
                    duration_ms,
                    status,
                } => {
                    let data = serde_json::json!({
                        "tool_name": tool_name,
                        "input": serde_json::from_str::<serde_json::Value>(input_json)
                            .unwrap_or(serde_json::Value::String(input_json.clone())),
                        "output": serde_json::from_str::<serde_json::Value>(output_json)
                            .unwrap_or(serde_json::Value::String(output_json.clone())),
                        "duration_ms": duration_ms,
                        "status": status.clone(),
                    });
                    (
                        "tool_call",
                        format!("{} ({}ms)", tool_name, duration_ms),
                        Some(serde_json::to_string(&data).ok()),
                    )
                }
                TraceEventKind::FileChange { path, action } => {
                    // FileChange events go to file_changes table, not traces.
                    let file_change_id = Uuid::new_v4().to_string();
                    conn.execute(
                        "INSERT INTO file_changes (id, trace_id, session_id, path, action, timestamp)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            file_change_id,
                            Option::<String>::None,
                            event.session_id,
                            path,
                            action.as_str(),
                            event.timestamp,
                        ],
                    )
                    ?;
                    continue;
                }
                TraceEventKind::Decision { text } => {
                    let data = serde_json::json!({ "text": text });
                    (
                        "decision",
                        truncate_summary(text, 200),
                        Some(serde_json::to_string(&data).ok()),
                    )
                }
                TraceEventKind::Error { message } => {
                    let data = serde_json::json!({ "message": message });
                    (
                        "error",
                        truncate_summary(message, 200),
                        Some(serde_json::to_string(&data).ok()),
                    )
                }
            };

            let data_str = data_json.flatten();
            conn.execute(
                "INSERT INTO traces (id, session_id, event_type, summary, data_json, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    trace_id,
                    event.session_id,
                    event_type,
                    summary,
                    data_str,
                    event.timestamp,
                ],
            )?;
        }

        Ok(())
    }

    /// Get all traces for a session, ordered by timestamp.
    pub fn list_by_session(db: &Db, session_id: &str) -> Result<Vec<TraceRow>, AppError> {
        let conn = db.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TRACE_COLS} FROM traces WHERE session_id = ?1 ORDER BY timestamp"
        ))?;

        let rows = stmt.query_map(rusqlite::params![session_id], Self::map_trace_row)?;

        let mut traces = Vec::new();
        for row in rows {
            traces.push(row?);
        }
        Ok(traces)
    }

    /// Get journey events (decision + error), ordered by timestamp.
    ///
    /// Other event types (milestone, review) are listed in the journey
    /// spec but not yet produced by `TraceEventKind`. Keep the filter
    /// honest to current producers; extend it when those variants land.
    pub fn list_journey(db: &Db, session_id: &str) -> Result<Vec<TraceRow>, AppError> {
        let conn = db.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TRACE_COLS} FROM traces
                 WHERE session_id = ?1
                   AND event_type IN ('decision', 'error')
                 ORDER BY timestamp"
        ))?;

        let rows = stmt.query_map(rusqlite::params![session_id], Self::map_trace_row)?;

        let mut traces = Vec::new();
        for row in rows {
            traces.push(row?);
        }
        Ok(traces)
    }

    /// Get tool call events for a session, ordered by timestamp.
    ///
    /// The Journey sidebar surfaces these as a third section ("Tools")
    /// so a session that only used tools (no decisions, no file edits)
    /// doesn't render as empty. Companion to `list_journey` /
    /// `list_file_changes`; same shape, different filter.
    pub fn list_tool_calls(db: &Db, session_id: &str) -> Result<Vec<TraceRow>, AppError> {
        let conn = db.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TRACE_COLS} FROM traces
                 WHERE session_id = ?1
                   AND event_type = 'tool_call'
                 ORDER BY timestamp"
        ))?;

        let rows = stmt.query_map(rusqlite::params![session_id], Self::map_trace_row)?;

        let mut traces = Vec::new();
        for row in rows {
            traces.push(row?);
        }
        Ok(traces)
    }

    /// Get deduplicated file changes for a session.
    ///
    /// Groups by path, collecting distinct actions and count.
    pub fn list_file_changes(
        db: &Db,
        session_id: &str,
    ) -> Result<Vec<FileChangeSummary>, AppError> {
        let conn = db.conn();
        let mut stmt = conn.prepare(
            "SELECT path, GROUP_CONCAT(DISTINCT action) as actions, COUNT(*) as cnt
                 FROM file_changes
                 WHERE session_id = ?1
                 GROUP BY path
                 ORDER BY MAX(timestamp) DESC",
        )?;

        let rows = stmt.query_map(rusqlite::params![session_id], |row| {
            let path: String = row.get(0)?;
            let actions_str: String = row.get(1)?;
            let count: u32 = row.get(2)?;
            Ok(FileChangeSummary {
                path,
                actions: actions_str.split(',').map(String::from).collect(),
                count,
            })
        })?;

        let mut changes = Vec::new();
        for row in rows {
            changes.push(row?);
        }
        Ok(changes)
    }

    fn map_trace_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TraceRow> {
        Ok(TraceRow {
            id: row.get(0)?,
            session_id: row.get(1)?,
            event_type: row.get(2)?,
            summary: row.get(3)?,
            data_json: row.get(4)?,
            timestamp: row.get(5)?,
        })
    }
}

/// Truncate a string to `max_len` chars, appending "…" if truncated.
fn truncate_summary(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find the largest char boundary <= max_len to avoid panicking on
        // multi-byte UTF-8 characters (CJK, emoji, accented chars).
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_db() -> Db {
        Db::open(Path::new(":memory:")).expect("failed to open test db")
    }

    fn seed_session(db: &Db) -> String {
        // Create workspace, provider, then session (FK: sessions.provider_id -> providers.id)
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params!["ws-1", "test", "2026-01-01", "2026-01-01"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO providers (id, type, name, config_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["prov-1", "anthropic", "test", "{}", "2026-01-01"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO sessions (id, workspace_id, provider_id, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    "sess-1",
                    "ws-1",
                    "prov-1",
                    "ready",
                    "2026-01-01",
                    "2026-01-01"
                ],
            )
            .unwrap();
        "sess-1".to_string()
    }

    #[test]
    fn test_insert_and_list_traces() {
        let db = test_db();
        let session_id = seed_session(&db);

        let events = vec![
            TraceEvent {
                session_id: session_id.clone(),
                kind: TraceEventKind::Decision {
                    text: "I will implement the feature".to_string(),
                },
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            TraceEvent {
                session_id: session_id.clone(),
                kind: TraceEventKind::ToolCall {
                    tool_name: "fs_write".to_string(),
                    input_json: r#"{"path":"/tmp/test.rs"}"#.to_string(),
                    output_json: r#"{"success":true}"#.to_string(),
                    duration_ms: 42,
                    status: None,
                },
                timestamp: "2026-01-01T00:00:01Z".to_string(),
            },
            TraceEvent {
                session_id: session_id.clone(),
                kind: TraceEventKind::Error {
                    message: "something went wrong".to_string(),
                },
                timestamp: "2026-01-01T00:00:02Z".to_string(),
            },
        ];

        TraceStore::insert_batch(&db, &events).unwrap();

        let traces = TraceStore::list_by_session(&db, &session_id).unwrap();
        assert_eq!(traces.len(), 3);
        assert_eq!(traces[0].event_type, "decision");
        assert_eq!(traces[1].event_type, "tool_call");
        assert_eq!(traces[2].event_type, "error");
        assert_eq!(traces[1].summary, "fs_write (42ms)");
    }

    #[test]
    fn test_insert_file_change_events() {
        let db = test_db();
        let session_id = seed_session(&db);

        let events = vec![TraceEvent {
            session_id: session_id.clone(),
            kind: TraceEventKind::FileChange {
                path: "/tmp/test.rs".to_string(),
                action: FileAction::Write,
            },
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        }];

        TraceStore::insert_batch(&db, &events).unwrap();

        // File changes go to file_changes table, not traces
        let traces = TraceStore::list_by_session(&db, &session_id).unwrap();
        assert_eq!(traces.len(), 0);

        let changes = TraceStore::list_file_changes(&db, &session_id).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "/tmp/test.rs");
        assert_eq!(changes[0].actions, vec!["write"]);
        assert_eq!(changes[0].count, 1);
    }

    #[test]
    fn test_journey_filters_event_types() {
        let db = test_db();
        let session_id = seed_session(&db);

        let events = vec![
            TraceEvent {
                session_id: session_id.clone(),
                kind: TraceEventKind::Decision {
                    text: "decided to use Rust".to_string(),
                },
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            TraceEvent {
                session_id: session_id.clone(),
                kind: TraceEventKind::ToolCall {
                    tool_name: "fs_read".to_string(),
                    input_json: "{}".to_string(),
                    output_json: "{}".to_string(),
                    duration_ms: 5,
                    status: None,
                },
                timestamp: "2026-01-01T00:00:01Z".to_string(),
            },
            TraceEvent {
                session_id: session_id.clone(),
                kind: TraceEventKind::Error {
                    message: "build failed".to_string(),
                },
                timestamp: "2026-01-01T00:00:02Z".to_string(),
            },
        ];

        TraceStore::insert_batch(&db, &events).unwrap();

        let journey = TraceStore::list_journey(&db, &session_id).unwrap();
        assert_eq!(journey.len(), 2);
        assert_eq!(journey[0].event_type, "decision");
        assert_eq!(journey[1].event_type, "error");
    }

    #[test]
    fn test_file_changes_deduplicated() {
        let db = test_db();
        let session_id = seed_session(&db);

        let events = vec![
            TraceEvent {
                session_id: session_id.clone(),
                kind: TraceEventKind::FileChange {
                    path: "/tmp/a.rs".to_string(),
                    action: FileAction::Write,
                },
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            TraceEvent {
                session_id: session_id.clone(),
                kind: TraceEventKind::FileChange {
                    path: "/tmp/a.rs".to_string(),
                    action: FileAction::Write,
                },
                timestamp: "2026-01-01T00:00:01Z".to_string(),
            },
            TraceEvent {
                session_id: session_id.clone(),
                kind: TraceEventKind::FileChange {
                    path: "/tmp/b.rs".to_string(),
                    action: FileAction::Create,
                },
                timestamp: "2026-01-01T00:00:02Z".to_string(),
            },
        ];

        TraceStore::insert_batch(&db, &events).unwrap();

        let changes = TraceStore::list_file_changes(&db, &session_id).unwrap();
        assert_eq!(changes.len(), 2);
        // a.rs appears twice but is deduplicated
        let a = changes.iter().find(|c| c.path == "/tmp/a.rs").unwrap();
        assert_eq!(a.count, 2);
        assert_eq!(a.actions, vec!["write"]);
    }

    #[test]
    fn test_empty_batch_is_noop() {
        let db = test_db();
        seed_session(&db);
        TraceStore::insert_batch(&db, &[]).unwrap();
    }

    #[test]
    fn test_truncate_summary() {
        assert_eq!(truncate_summary("short", 200), "short");
        let long = "a".repeat(300);
        let truncated = truncate_summary(&long, 200);
        assert!(truncated.len() <= 203); // 200 + "…" (3 bytes UTF-8)
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn test_truncate_summary_multibyte_utf8() {
        // CJK characters are 3 bytes each. A string of 100 CJK chars = 300 bytes.
        // Truncating at byte 200 must not panic — it should find a char boundary.
        let cjk: String = "日".repeat(100); // 300 bytes
        let truncated = truncate_summary(&cjk, 200);
        assert!(truncated.ends_with('…'));
        // Should truncate to 66 CJK chars (198 bytes) + "…" (3 bytes) = 201 bytes
        assert!(truncated.len() <= 203);
    }
}
