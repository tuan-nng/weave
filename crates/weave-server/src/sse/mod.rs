use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use serde::Serialize;

use crate::agent;

// ---------------------------------------------------------------------------
// SseManager — per-entity broadcast channels + event buffer
// ---------------------------------------------------------------------------

/// Manages SSE broadcast channels and event buffers for entities (sessions, boards).
///
/// Each entity gets a `tokio::broadcast` channel and a ring buffer of recent
/// events. New subscribers receive buffered events first (for reconnection
/// replay), then live events from the broadcast channel.
pub struct SseManager {
    channels: RwLock<HashMap<String, tokio::sync::broadcast::Sender<SseWireEvent>>>,
    buffer: EventBuffer,
    counters: RwLock<HashMap<String, AtomicU64>>,
}

impl SseManager {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            buffer: EventBuffer::new(100),
            counters: RwLock::new(HashMap::new()),
        }
    }

    /// Subscribe to an entity's event stream.
    ///
    /// If no channel exists for this entity, one is created lazily.
    /// The returned receiver will only see events broadcast *after* this call.
    /// Use [`SseManager::get_after`] to retrieve buffered events from before
    /// the subscription point.
    pub fn subscribe(&self, entity_id: &str) -> tokio::sync::broadcast::Receiver<SseWireEvent> {
        let mut channels = self.channels.write().expect("sse channels lock poisoned");
        match channels.get(entity_id) {
            Some(tx) => tx.subscribe(),
            None => {
                let (tx, _rx) = tokio::sync::broadcast::channel(256);
                let rx = tx.subscribe();
                channels.insert(entity_id.to_string(), tx);
                // Initialize counter for this entity (IDs start at 1)
                let mut counters = self.counters.write().expect("sse counters lock poisoned");
                counters.insert(entity_id.to_string(), AtomicU64::new(1));
                rx
            }
        }
    }

    /// Broadcast an event to all subscribers of an entity.
    ///
    /// Returns the monotonically increasing event ID. The event is also
    /// stored in the ring buffer for reconnection replay.
    pub fn broadcast(&self, entity_id: &str, event: SseWireEvent) -> u64 {
        // Use write lock for atomic check-and-create (avoids TOCTOU race)
        let mut channels = self.channels.write().expect("sse channels lock poisoned");
        let tx = channels
            .entry(entity_id.to_string())
            .or_insert_with(|| {
                let (tx, _rx) = tokio::sync::broadcast::channel(256);
                // Initialize counter for this entity (IDs start at 1)
                let mut counters = self.counters.write().expect("sse counters lock poisoned");
                counters.insert(entity_id.to_string(), AtomicU64::new(1));
                tx
            })
            .clone();
        drop(channels);

        // Increment counter
        let id = {
            let counters = self.counters.read().expect("sse counters lock poisoned");
            counters
                .get(entity_id)
                .map(|c| c.fetch_add(1, Ordering::Relaxed))
                .unwrap_or(1)
        };

        // Buffer the event
        self.buffer.push(entity_id, id, &event);

        // Broadcast (ignore if no receivers)
        let _ = tx.send(event);

        id
    }

    /// Retrieve buffered events with ID greater than `after_id`.
    ///
    /// Used for `Last-Event-ID` reconnection replay.
    pub fn get_after(&self, entity_id: &str, after_id: u64) -> Vec<BufferedEvent> {
        self.buffer.get_after(entity_id, after_id)
    }

    /// Get the current (next) event ID for an entity.
    ///
    /// Returns the ID that will be assigned to the next broadcast event.
    /// Used for deduplication when transitioning from buffered to live events.
    pub fn get_current_id(&self, entity_id: &str) -> u64 {
        let counters = self.counters.read().expect("sse counters lock poisoned");
        counters
            .get(entity_id)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(1)
    }

    /// Broadcast a shutdown event to every live channel.
    ///
    /// Iterates all known entity channels (lazily created by `subscribe` or
    /// `broadcast`) and pushes a `Shutdown` event into each. Returns the
    /// number of entities notified. The event is also stored in the per-entity
    /// ring buffer so a client that reconnects after the server has already
    /// sent `Shutdown` will still see the disconnect notice on replay.
    ///
    /// Used by the graceful-shutdown sequence (feat-034) to tell every SSE
    /// client to close their EventSource before the server returns. A
    /// shutdown with no live subscribers is a no-op.
    pub fn broadcast_shutdown(&self, reason: &str) -> usize {
        // Snapshot the entity ids under the read lock so we don't hold it
        // across the per-entity `broadcast` call (which takes the write lock).
        let entity_ids: Vec<String> = {
            let channels = self.channels.read().expect("sse channels lock poisoned");
            channels.keys().cloned().collect()
        };

        for entity_id in &entity_ids {
            self.broadcast(
                entity_id,
                SseWireEvent::Shutdown {
                    reason: reason.to_string(),
                },
            );
        }

        entity_ids.len()
    }
}

// ---------------------------------------------------------------------------
// EventBuffer — ring buffer per entity
// ---------------------------------------------------------------------------

/// Per-entity ring buffer of recent SSE events for reconnection replay.
struct EventBuffer {
    buffers: RwLock<HashMap<String, VecDeque<BufferedEvent>>>,
    max_size: usize,
}

impl EventBuffer {
    fn new(max_size: usize) -> Self {
        Self {
            buffers: RwLock::new(HashMap::new()),
            max_size,
        }
    }

    fn push(&self, entity_id: &str, id: u64, event: &SseWireEvent) {
        let entry = BufferedEvent {
            id,
            event_type: event.event_type().to_string(),
            data: serde_json::to_string(event).unwrap_or_default(),
        };
        let mut buffers = self.buffers.write().expect("sse buffer lock poisoned");
        let buf = buffers.entry(entity_id.to_string()).or_default();
        if buf.len() >= self.max_size {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    fn get_after(&self, entity_id: &str, after_id: u64) -> Vec<BufferedEvent> {
        let buffers = self.buffers.read().expect("sse buffer lock poisoned");
        buffers
            .get(entity_id)
            .map(|buf| buf.iter().filter(|e| e.id > after_id).cloned().collect())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// An event stored in the ring buffer with its monotonic ID.
#[derive(Debug, Clone)]
pub struct BufferedEvent {
    pub id: u64,
    pub event_type: String,
    pub data: String,
}

/// SSE wire event — either an agent stream event or an SSE-protocol event.
///
/// Agent events are serialized as their `StreamEvent` JSON (e.g.,
/// `{"type":"text_delta","text":"Hello"}`). SSE-protocol events have
/// custom payloads.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseWireEvent {
    // Agent stream events (forwarded from StreamEvent)
    TextDelta {
        text: String,
    },
    ToolUseStart {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolUseDelta {
        id: String,
        delta: String,
    },
    ToolResult {
        id: String,
        result: String,
    },
    Thinking {
        text: String,
    },
    Done {
        stop_reason: agent::StopReason,
    },
    Error {
        message: String,
    },
    /// Emitted exactly once per assistant turn, AFTER the assistant message
    /// has been persisted to the database and BEFORE the terminal `done`
    /// event. The frontend uses the `id` to swap the live streaming bubble
    /// for the persisted one — replacing the old content-equality dedup
    /// with a precise id-based handoff. `id == ""` is the sentinel for
    /// "no message was persisted this turn" (e.g. cancel before any text,
    /// or an empty accumulator).
    MessagePersisted {
        id: String,
        role: String,
        stop_reason: Option<String>,
        content: String,
        created_at: String,
    },
    // SSE-protocol events
    Connected {
        session_id: String,
    },
    Gap {
        missed: u64,
    },
    // Kanban board events (feat-025). Broadcast on entity_id `board:{bid}`.
    // Task and column payloads use `serde_json::Value` so this enum stays
    // decoupled from the `store::*` types — the kanban API serializes the
    // full struct once and passes the JSON.
    TaskCreated {
        task: serde_json::Value,
    },
    TaskMoved {
        task: serde_json::Value,
        from_column_id: String,
        to_column_id: String,
    },
    TaskUpdated {
        task: serde_json::Value,
    },
    TaskDeleted {
        task_id: String,
        column_id: String,
    },
    ColumnAdded {
        column: serde_json::Value,
    },
    SessionStarted {
        session_id: String,
        task_id: String,
        specialist_id: String,
        board_id: String,
    },
    Heartbeat {},
    /// Server-initiated disconnect notice. Broadcast to all live subscribers
    /// when the server begins a graceful shutdown (feat-034). The `reason`
    /// is `"sigterm"`, `"sigint"`, or a test marker; clients should close
    /// their EventSource on receipt. Distinct from `Connected` (sent on
    /// subscribe) and `Gap` (sent on replay overflow) — this is a
    /// server-initiated "the stream is ending now" event.
    Shutdown {
        reason: String,
    },
}

impl SseWireEvent {
    /// The SSE `event:` type string for this wire event.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::TextDelta { .. } => "text_delta",
            Self::ToolUseStart { .. } => "tool_use_start",
            Self::ToolUseDelta { .. } => "tool_use_delta",
            Self::ToolResult { .. } => "tool_result",
            Self::Thinking { .. } => "thinking",
            Self::Done { .. } => "done",
            Self::Error { .. } => "error",
            Self::MessagePersisted { .. } => "message_persisted",
            Self::Connected { .. } => "connected",
            Self::Gap { .. } => "gap",
            Self::TaskCreated { .. } => "task_created",
            Self::TaskMoved { .. } => "task_moved",
            Self::TaskUpdated { .. } => "task_updated",
            Self::TaskDeleted { .. } => "task_deleted",
            Self::ColumnAdded { .. } => "column_added",
            Self::SessionStarted { .. } => "session_started",
            Self::Heartbeat { .. } => "heartbeat",
            Self::Shutdown { .. } => "shutdown",
        }
    }
}

/// Convert an agent `StreamEvent` into an `SseWireEvent`.
pub fn stream_event_to_wire(event: agent::StreamEvent) -> SseWireEvent {
    match event {
        agent::StreamEvent::TextDelta { text } => SseWireEvent::TextDelta { text },
        agent::StreamEvent::ToolUseStart { id, name, input } => {
            SseWireEvent::ToolUseStart { id, name, input }
        }
        agent::StreamEvent::ToolUseDelta { id, delta } => SseWireEvent::ToolUseDelta { id, delta },
        agent::StreamEvent::ToolResult { id, result } => SseWireEvent::ToolResult { id, result },
        agent::StreamEvent::Thinking { text } => SseWireEvent::Thinking { text },
        agent::StreamEvent::Done { stop_reason } => SseWireEvent::Done { stop_reason },
        agent::StreamEvent::Error { message } => SseWireEvent::Error { message },
    }
}

/// Serialize an `SseWireEvent` to the SSE `data:` line content.
pub fn sse_data(event: &SseWireEvent) -> String {
    serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_broadcast_and_subscribe() {
        let manager = SseManager::new();
        let mut rx = manager.subscribe("session-1");

        manager.broadcast(
            "session-1",
            SseWireEvent::TextDelta {
                text: "hello".into(),
            },
        );

        let event = rx.recv().await.unwrap();
        match event {
            SseWireEvent::TextDelta { text } => assert_eq!(text, "hello"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[tokio::test]
    async fn test_event_ids_monotonically_increasing() {
        let manager = SseManager::new();

        let id0 = manager.broadcast("session-1", SseWireEvent::TextDelta { text: "a".into() });
        let id1 = manager.broadcast("session-1", SseWireEvent::TextDelta { text: "b".into() });
        let id2 = manager.broadcast("session-1", SseWireEvent::TextDelta { text: "c".into() });

        assert_eq!(id0, 1);
        assert_eq!(id1, 2);
        assert_eq!(id2, 3);
    }

    #[tokio::test]
    async fn test_event_buffer_replay() {
        let manager = SseManager::new();

        // Broadcast some events before any subscriber (IDs 1-5)
        for i in 0..5 {
            manager.broadcast(
                "session-1",
                SseWireEvent::TextDelta {
                    text: format!("msg-{}", i),
                },
            );
        }

        // Client reconnects with Last-Event-ID = 2 (wants events after id 2)
        let replayed = manager.get_after("session-1", 2);
        assert_eq!(replayed.len(), 3); // ids 3, 4, 5
        assert_eq!(replayed[0].id, 3);
        assert_eq!(replayed[1].id, 4);
        assert_eq!(replayed[2].id, 5);
    }

    #[tokio::test]
    async fn test_event_buffer_ring_eviction() {
        let manager = SseManager::new();

        // Fill buffer beyond max (100)
        for i in 0..105 {
            manager.broadcast(
                "session-1",
                SseWireEvent::TextDelta {
                    text: format!("msg-{}", i),
                },
            );
        }

        // Oldest events should be evicted (IDs 1-5 evicted, 6-105 remain)
        let all = manager.get_after("session-1", 0);
        assert_eq!(all.len(), 100);
        assert_eq!(all[0].id, 6); // first 5 (IDs 1-5) were evicted
    }

    #[tokio::test]
    async fn test_multiple_entities_independent() {
        let manager = SseManager::new();
        let mut rx1 = manager.subscribe("session-1");
        let mut rx2 = manager.subscribe("session-2");

        manager.broadcast(
            "session-1",
            SseWireEvent::TextDelta {
                text: "for-1".into(),
            },
        );
        manager.broadcast(
            "session-2",
            SseWireEvent::TextDelta {
                text: "for-2".into(),
            },
        );

        let event1 = rx1.recv().await.unwrap();
        let event2 = rx2.recv().await.unwrap();

        match event1 {
            SseWireEvent::TextDelta { text } => assert_eq!(text, "for-1"),
            _ => panic!("expected TextDelta"),
        }
        match event2 {
            SseWireEvent::TextDelta { text } => assert_eq!(text, "for-2"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[tokio::test]
    async fn test_sse_wire_event_types() {
        // Verify event_type() returns correct SSE event names
        assert_eq!(
            SseWireEvent::TextDelta { text: "".into() }.event_type(),
            "text_delta"
        );
        assert_eq!(
            SseWireEvent::Done {
                stop_reason: agent::StopReason::EndTurn
            }
            .event_type(),
            "done"
        );
        assert_eq!(
            SseWireEvent::Error { message: "".into() }.event_type(),
            "error"
        );
        assert_eq!(
            SseWireEvent::Connected {
                session_id: "".into()
            }
            .event_type(),
            "connected"
        );
        assert_eq!(SseWireEvent::Gap { missed: 0 }.event_type(), "gap");
        assert_eq!(
            SseWireEvent::MessagePersisted {
                id: "x".into(),
                role: "assistant".into(),
                stop_reason: Some("end_turn".into()),
                content: "hi".into(),
                created_at: "2026-06-01T00:00:00Z".into(),
            }
            .event_type(),
            "message_persisted"
        );
    }

    /// `Shutdown` event uses the `shutdown` SSE event type so the frontend
    /// can dispatch it without parsing the data payload.
    #[tokio::test]
    async fn test_sse_wire_event_shutdown_type() {
        assert_eq!(
            SseWireEvent::Shutdown {
                reason: "sigterm".into()
            }
            .event_type(),
            "shutdown"
        );
    }

    /// `broadcast_shutdown` fans out a `Shutdown` event to every live
    /// subscriber across all entities. Subscribers see the event with the
    /// exact reason that was passed in.
    #[tokio::test]
    async fn test_broadcast_shutdown_fans_out_to_all_subscribers() {
        let manager = SseManager::new();
        let mut rx_session = manager.subscribe("session-1");
        let mut rx_board = manager.subscribe("board-42");

        let count = manager.broadcast_shutdown("sigterm");
        assert_eq!(count, 2, "both entities should be notified");

        let event_session = rx_session.recv().await.expect("session subscriber");
        let event_board = rx_board.recv().await.expect("board subscriber");

        match event_session {
            SseWireEvent::Shutdown { reason } => assert_eq!(reason, "sigterm"),
            other => panic!("expected Shutdown, got {other:?}"),
        }
        match event_board {
            SseWireEvent::Shutdown { reason } => assert_eq!(reason, "sigterm"),
            other => panic!("expected Shutdown, got {other:?}"),
        }
    }

    /// A shutdown with no live channels is a no-op — it does not panic,
    /// does not allocate, and returns 0.
    #[tokio::test]
    async fn test_broadcast_shutdown_no_subscribers_is_noop() {
        let manager = SseManager::new();
        let count = manager.broadcast_shutdown("sigint");
        assert_eq!(count, 0, "no entities should be notified");
    }

    #[tokio::test]
    async fn test_message_persisted_serde_roundtrip() {
        let event = SseWireEvent::MessagePersisted {
            id: "550e8400-e29b-41d4-a716-446655440000".into(),
            role: "assistant".into(),
            stop_reason: Some("end_turn".into()),
            content: "Hello, world".into(),
            created_at: "2026-06-01T20:14:33.512Z".into(),
        };
        let json = sse_data(&event);
        // JSON shape: {"type":"message_persisted","id":...,"role":...,
        //   "stop_reason":...,"content":...,"created_at":...}
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], "message_persisted");
        assert_eq!(value["id"], "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(value["role"], "assistant");
        assert_eq!(value["stop_reason"], "end_turn");
        assert_eq!(value["content"], "Hello, world");
        assert_eq!(value["created_at"], "2026-06-01T20:14:33.512Z");
    }

    #[tokio::test]
    async fn test_message_persisted_sentinel_for_empty_turn() {
        // When the turn produced no accumulated text, the persisted event
        // still fires with id="" so the frontend knows the live bubble
        // is no longer the latest. The stop_reason is still meaningful.
        let event = SseWireEvent::MessagePersisted {
            id: String::new(),
            role: "assistant".into(),
            stop_reason: Some("cancelled".into()),
            content: String::new(),
            created_at: "2026-06-01T20:14:33.512Z".into(),
        };
        assert_eq!(event.event_type(), "message_persisted");
        let json = sse_data(&event);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["id"], "");
        assert_eq!(value["stop_reason"], "cancelled");
        assert_eq!(value["content"], "");
    }

    #[tokio::test]
    async fn test_message_persisted_broadcast_and_receive() {
        // The new event flows through SseManager like any other wire
        // event — assigned an id, buffered, and delivered to subscribers.
        let manager = SseManager::new();
        let mut rx = manager.subscribe("session-1");

        let assigned_id = manager.broadcast(
            "session-1",
            SseWireEvent::MessagePersisted {
                id: "msg-1".into(),
                role: "assistant".into(),
                stop_reason: Some("end_turn".into()),
                content: "hi".into(),
                created_at: "2026-06-01T20:14:33.512Z".into(),
            },
        );
        assert!(assigned_id > 0);

        let received = rx.recv().await.unwrap();
        match received {
            SseWireEvent::MessagePersisted {
                id,
                content,
                stop_reason,
                ..
            } => {
                assert_eq!(id, "msg-1");
                assert_eq!(content, "hi");
                assert_eq!(stop_reason, Some("end_turn".into()));
            }
            _ => panic!("expected MessagePersisted"),
        }
    }

    #[tokio::test]
    async fn test_stream_event_to_wire_conversion() {
        let wire = stream_event_to_wire(agent::StreamEvent::TextDelta {
            text: "hello".into(),
        });
        match wire {
            SseWireEvent::TextDelta { text } => assert_eq!(text, "hello"),
            _ => panic!("expected TextDelta"),
        }

        let wire = stream_event_to_wire(agent::StreamEvent::Done {
            stop_reason: agent::StopReason::EndTurn,
        });
        match wire {
            SseWireEvent::Done { stop_reason } => {
                assert_eq!(stop_reason, agent::StopReason::EndTurn)
            }
            _ => panic!("expected Done"),
        }
    }

    // --- feat-025: kanban board event tests ---

    #[test]
    fn test_kanban_event_types() {
        let task_json = serde_json::json!({"id": "t-1", "title": "T"});
        let col_json = serde_json::json!({"id": "c-1", "name": "C"});
        assert_eq!(
            SseWireEvent::TaskCreated {
                task: task_json.clone()
            }
            .event_type(),
            "task_created"
        );
        assert_eq!(
            SseWireEvent::TaskMoved {
                task: task_json.clone(),
                from_column_id: "c1".into(),
                to_column_id: "c2".into(),
            }
            .event_type(),
            "task_moved"
        );
        assert_eq!(
            SseWireEvent::TaskUpdated {
                task: task_json.clone()
            }
            .event_type(),
            "task_updated"
        );
        assert_eq!(
            SseWireEvent::TaskDeleted {
                task_id: "t".into(),
                column_id: "c".into(),
            }
            .event_type(),
            "task_deleted"
        );
        assert_eq!(
            SseWireEvent::ColumnAdded { column: col_json }.event_type(),
            "column_added"
        );
        assert_eq!(
            SseWireEvent::SessionStarted {
                session_id: "s".into(),
                task_id: "t".into(),
                specialist_id: "dev".into(),
                board_id: "b".into(),
            }
            .event_type(),
            "session_started"
        );
        assert_eq!(SseWireEvent::Heartbeat {}.event_type(), "heartbeat");
    }

    #[test]
    fn test_kanban_event_serde_shapes() {
        // Each variant serializes to {"type":"<name>", ...} with the
        // expected payload fields. Frontend distinguishes by `type`.
        let task_json = serde_json::json!({"id": "t-1", "title": "T"});
        let col_json = serde_json::json!({"id": "c-1", "name": "C"});

        let v: serde_json::Value = serde_json::from_str(&sse_data(&SseWireEvent::TaskCreated {
            task: task_json.clone(),
        }))
        .unwrap();
        assert_eq!(v["type"], "task_created");
        assert_eq!(v["task"]["id"], "t-1");

        let v: serde_json::Value = serde_json::from_str(&sse_data(&SseWireEvent::TaskMoved {
            task: task_json.clone(),
            from_column_id: "c1".into(),
            to_column_id: "c2".into(),
        }))
        .unwrap();
        assert_eq!(v["type"], "task_moved");
        assert_eq!(v["from_column_id"], "c1");
        assert_eq!(v["to_column_id"], "c2");

        let v: serde_json::Value = serde_json::from_str(&sse_data(&SseWireEvent::TaskUpdated {
            task: task_json.clone(),
        }))
        .unwrap();
        assert_eq!(v["type"], "task_updated");
        assert_eq!(v["task"]["title"], "T");

        let v: serde_json::Value = serde_json::from_str(&sse_data(&SseWireEvent::TaskDeleted {
            task_id: "t-1".into(),
            column_id: "c-1".into(),
        }))
        .unwrap();
        assert_eq!(v["type"], "task_deleted");
        assert_eq!(v["task_id"], "t-1");
        assert_eq!(v["column_id"], "c-1");

        let v: serde_json::Value =
            serde_json::from_str(&sse_data(&SseWireEvent::ColumnAdded { column: col_json }))
                .unwrap();
        assert_eq!(v["type"], "column_added");
        assert_eq!(v["column"]["name"], "C");

        let v: serde_json::Value = serde_json::from_str(&sse_data(&SseWireEvent::SessionStarted {
            session_id: "s-1".into(),
            task_id: "t-1".into(),
            specialist_id: "dev".into(),
            board_id: "b-1".into(),
        }))
        .unwrap();
        assert_eq!(v["type"], "session_started");
        assert_eq!(v["session_id"], "s-1");
        assert_eq!(v["task_id"], "t-1");
        assert_eq!(v["specialist_id"], "dev");
        assert_eq!(v["board_id"], "b-1");

        // Heartbeat is the empty-payload sentinel — the data line is
        // exactly `{"type":"heartbeat"}`.
        let v: serde_json::Value =
            serde_json::from_str(&sse_data(&SseWireEvent::Heartbeat {})).unwrap();
        assert_eq!(v, serde_json::json!({"type": "heartbeat"}));
    }

    #[tokio::test]
    async fn test_session_started_broadcast_and_receive() {
        let manager = SseManager::new();
        let mut rx = manager.subscribe("board:b-1");

        let id = manager.broadcast(
            "board:b-1",
            SseWireEvent::SessionStarted {
                session_id: "s-1".into(),
                task_id: "t-1".into(),
                specialist_id: "dev".into(),
                board_id: "b-1".into(),
            },
        );
        assert_eq!(id, 1);

        let received = rx.recv().await.unwrap();
        match received {
            SseWireEvent::SessionStarted {
                session_id,
                task_id,
                specialist_id,
                board_id,
            } => {
                assert_eq!(session_id, "s-1");
                assert_eq!(task_id, "t-1");
                assert_eq!(specialist_id, "dev");
                assert_eq!(board_id, "b-1");
            }
            _ => panic!("expected SessionStarted"),
        }
    }
}
