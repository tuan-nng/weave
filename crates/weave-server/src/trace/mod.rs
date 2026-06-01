//! Trace collection and persistence.
//!
//! The `TraceCollector` is the sender-side handle that the session streaming
//! loop uses to emit events. The background flush task receives events through
//! an mpsc channel and batch-inserts them into SQLite via `TraceStore`.

use std::sync::Arc;

use crate::db::Db;
use crate::store::traces::{FileAction, TraceEvent, TraceEventKind, TraceStore};

// ---------------------------------------------------------------------------
// TraceCollector (sender-side handle)
// ---------------------------------------------------------------------------

/// Collects trace events during agent execution.
///
/// Holds the sender half of an mpsc channel. The receiver half is owned by
/// the background flush task. Constructed once per session in `run_prompt_task`.
pub struct TraceCollector {
    tx: tokio::sync::mpsc::UnboundedSender<TraceEvent>,
}

impl TraceCollector {
    /// Create a new collector from the given sender.
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<TraceEvent>) -> Self {
        Self { tx }
    }

    /// Emit a trace event. Never blocks; drops the event if the channel is closed.
    pub fn emit(&self, event: TraceEvent) {
        let _ = self.tx.send(event);
    }
}

// ---------------------------------------------------------------------------
// File change extraction
// ---------------------------------------------------------------------------

/// Extract file change events from a tool call's input.
///
/// Returns an empty vec for tools that don't modify files.
/// Called in the session streaming loop when `ToolResult` arrives.
pub fn extract_file_changes(
    session_id: &str,
    tool_name: &str,
    input: &serde_json::Value,
    timestamp: &str,
) -> Vec<TraceEvent> {
    let paths: Vec<(&str, FileAction)> = match tool_name {
        "fs_write" => {
            if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                vec![(path, FileAction::Write)]
            } else {
                vec![]
            }
        }
        "fs_edit" => {
            if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                vec![(path, FileAction::Write)]
            } else {
                vec![]
            }
        }
        // git_commit doesn't specify individual files in input;
        // file changes are implicit from staging. Not extracted here.
        _ => vec![],
    };

    paths
        .into_iter()
        .map(|(path, action)| TraceEvent {
            session_id: session_id.to_string(),
            kind: TraceEventKind::FileChange {
                path: path.to_string(),
                action,
            },
            timestamp: timestamp.to_string(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Background flush task
// ---------------------------------------------------------------------------

/// Spawn the background flush task for a session.
///
/// Receives `TraceEvent` values from the channel and batch-inserts them into
/// SQLite. The task exits when the sender is dropped (channel closed).
/// Returns the `TraceCollector` (sender) and the task's `JoinHandle`.
pub fn spawn_flush_task(db: Arc<Db>) -> (TraceCollector, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TraceEvent>();
    let collector = TraceCollector::new(tx);

    let handle = tokio::spawn(async move {
        let mut buffer: Vec<TraceEvent> = Vec::with_capacity(32);
        let flush_interval = tokio::time::Duration::from_millis(200);
        let mut interval = tokio::time::interval(flush_interval);

        loop {
            tokio::select! {
                received = rx.recv() => {
                    match received {
                        Some(event) => {
                            buffer.push(event);
                            // Drain any additional queued events
                            while let Ok(event) = rx.try_recv() {
                                buffer.push(event);
                            }
                            // Flush if buffer is large enough
                            if buffer.len() >= 50 {
                                flush(&db, &mut buffer);
                            }
                        }
                        None => {
                            // Channel closed — flush remaining and exit
                            if !buffer.is_empty() {
                                flush(&db, &mut buffer);
                            }
                            break;
                        }
                    }
                }
                _ = interval.tick() => {
                    if !buffer.is_empty() {
                        flush(&db, &mut buffer);
                    }
                }
            }
        }
    });

    (collector, handle)
}

/// Flush the buffer to SQLite.
fn flush(db: &Db, buffer: &mut Vec<TraceEvent>) {
    if let Err(e) = TraceStore::insert_batch(db, buffer) {
        tracing::error!(error = %e, "failed to flush trace events");
    }
    buffer.clear();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_db() -> Db {
        Db::open(Path::new(":memory:")).expect("failed to open test db")
    }

    fn seed_session(db: &Db) -> String {
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
    fn test_extract_file_changes_fs_write() {
        let input = serde_json::json!({"path": "/tmp/test.rs", "content": "fn main() {}"});
        let events = extract_file_changes("sess-1", "fs_write", &input, "2026-01-01T00:00:00Z");
        assert_eq!(events.len(), 1);
        match &events[0].kind {
            TraceEventKind::FileChange { path, action } => {
                assert_eq!(path, "/tmp/test.rs");
                assert_eq!(action.as_str(), "write");
            }
            _ => panic!("expected FileChange"),
        }
    }

    #[test]
    fn test_extract_file_changes_fs_edit() {
        let input =
            serde_json::json!({"path": "/tmp/test.rs", "old_string": "a", "new_string": "b"});
        let events = extract_file_changes("sess-1", "fs_edit", &input, "2026-01-01T00:00:00Z");
        assert_eq!(events.len(), 1);
        match &events[0].kind {
            TraceEventKind::FileChange { path, action } => {
                assert_eq!(path, "/tmp/test.rs");
                assert_eq!(action.as_str(), "write");
            }
            _ => panic!("expected FileChange"),
        }
    }

    #[test]
    fn test_extract_file_changes_unknown_tool() {
        let input = serde_json::json!({});
        let events = extract_file_changes("sess-1", "shell_exec", &input, "2026-01-01T00:00:00Z");
        assert!(events.is_empty());
    }

    #[test]
    fn test_extract_file_changes_missing_path() {
        let input = serde_json::json!({"content": "hello"});
        let events = extract_file_changes("sess-1", "fs_write", &input, "2026-01-01T00:00:00Z");
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_flush_task_receives_and_persists() {
        let db = Arc::new(test_db());
        let session_id = seed_session(&db);

        let (collector, handle) = spawn_flush_task(db.clone());

        // Emit some events
        collector.emit(TraceEvent {
            session_id: session_id.clone(),
            kind: TraceEventKind::Decision {
                text: "test decision".to_string(),
            },
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        });
        collector.emit(TraceEvent {
            session_id: session_id.clone(),
            kind: TraceEventKind::ToolCall {
                tool_name: "fs_read".to_string(),
                input_json: "{}".to_string(),
                output_json: "{}".to_string(),
                duration_ms: 10,
            },
            timestamp: "2026-01-01T00:00:01Z".to_string(),
        });

        // Drop the collector to close the channel
        drop(collector);

        // Wait for the flush task to finish
        handle.await.unwrap();

        // Verify events were persisted
        let traces = TraceStore::list_by_session(&db, &session_id).unwrap();
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0].event_type, "decision");
        assert_eq!(traces[1].event_type, "tool_call");
    }

    #[tokio::test]
    async fn test_flush_task_handles_file_changes() {
        let db = Arc::new(test_db());
        let session_id = seed_session(&db);

        let (collector, handle) = spawn_flush_task(db.clone());

        collector.emit(TraceEvent {
            session_id: session_id.clone(),
            kind: TraceEventKind::FileChange {
                path: "/tmp/test.rs".to_string(),
                action: FileAction::Write,
            },
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        });

        drop(collector);
        handle.await.unwrap();

        let changes = TraceStore::list_file_changes(&db, &session_id).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "/tmp/test.rs");
    }

    #[test]
    fn test_collector_never_blocks() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let collector = TraceCollector::new(tx);

        // Emit works even if nobody is listening (event is queued or dropped)
        collector.emit(TraceEvent {
            session_id: "test".to_string(),
            kind: TraceEventKind::Decision {
                text: "test".to_string(),
            },
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        });
    }
}
