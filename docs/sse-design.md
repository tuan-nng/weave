# SSE Design

Server-Sent Events infrastructure for real-time streaming. SSE is the **only** real-time transport — no WebSocket.

## Server-Side Architecture

```rust
pub struct SseManager {
    // Per-entity broadcast channels
    session_streams: RwLock<HashMap<String, broadcast::Sender<ServerEvent>>>,
    board_streams: RwLock<HashMap<String, broadcast::Sender<KanbanEvent>>>,
}

impl SseManager {
    pub fn create_stream(&self, entity_id: &str) -> broadcast::Sender<ServerEvent> {
        let (tx, _) = broadcast::channel(256);
        self.session_streams.write().insert(entity_id.to_string(), tx.clone());
        tx
    }

    pub fn subscribe(&self, entity_id: &str) -> Option<broadcast::Receiver<ServerEvent>> {
        self.session_streams.read().get(entity_id).map(|tx| tx.subscribe())
    }

    pub fn broadcast(&self, entity_id: &str, event: ServerEvent) {
        if let Some(tx) = self.session_streams.read().get(entity_id) {
            let _ = tx.send(event); // Ignore if no receivers
        }
    }
}
```

## SSE Endpoint Handler

```rust
async fn session_stream(
    Path(session_id): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // 1. Verify session exists
    // 2. Get or create broadcast channel
    // 3. If Last-Event-ID header present, replay buffered events
    // 4. Return SSE response stream

    let last_event_id = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let receiver = state.sse_manager.subscribe(&session_id);

    Sse::new(stream! {
        // Send connected event
        yield Event::default()
            .event("connected")
            .data(json!({"session_id": session_id}).to_string())
            .id("0");

        // Replay missed events if reconnecting
        if let Some(last_id) = last_event_id {
            for event in state.event_buffer.get_after(&session_id, last_id) {
                yield event.to_sse_event();
            }
        }

        // Stream live events
        while let Ok(event) = receiver.recv().await {
            yield event.to_sse_event();
        }
    })
    .keep_alive(KeepAlive::new(Duration::from_secs(15)))
}
```

## Event Buffer

```rust
pub struct EventBuffer {
    buffers: RwLock<HashMap<String, VecDeque<BufferedEvent>>>,
    max_size: usize,  // 100 events per entity
}

struct BufferedEvent {
    id: u64,
    event_type: String,
    data: String,
}
```

Each entity (session, board) maintains a ring buffer of the last 100 events. On reconnection, events after `Last-Event-ID` are replayed.

## Connection Lifecycle and Failure Behavior

**Server restart:**
- All SSE connections drop. In-memory event buffers are lost.
- Active agent sessions are cancelled (graceful shutdown).
- On reconnect, clients receive a `connected` event with no replay.
- Frontend detects reconnect and refetches session state via REST.

**Client disconnect:**
- SSE stream generator exits. No cleanup needed — broadcast channels auto-drop subscribers.
- Agent session continues running (spawned tokio task is independent of SSE connection).
- Client can reconnect later and resume receiving events from the buffer.

**Backpressure:**
- `broadcast::channel(256)` — if a subscriber is slow, oldest unread events are dropped (`RecvError::Lagged`).
- SSE handler detects lag and sends a `gap` event: `{"event": "gap", "missed": N}`.
- Frontend responds by refetching session state via REST to re-sync.

**Max concurrent SSE connections:** No hard limit in v1. Bottleneck is OS file descriptors (default 1024). For production, set `ulimit -n 4096` or higher. Each SSE connection holds one TCP socket and one tokio task.
