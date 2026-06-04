# sse/ — Server-Sent Events Infrastructure

Real-time event broadcasting via SSE. Single file: `mod.rs` (24KB, ~700 lines).

## Key Types

| Type | Purpose |
|------|---------|
| `SseManager` | Central broadcast hub — `subscribe(entity_id)` returns a receiver, `broadcast(entity_id, event)` sends to all subscribers. One per AppState. |
| `EventBuffer` | Ring buffer (500 events per entity) for replay on reconnect. Uses `Last-Event-ID` header. |
| `BufferedEvent` | Wrapper: event + monotonic event ID for replay ordering |
| `SseWireEvent` | Serialized SSE event: `event_type`, `data`, `id`, `retry_ms`. The on-the-wire format. |

## Entity Types

SSE channels are keyed by `(entity_type, entity_id)`:
- `session` — session streaming (agent output, tool use, message persistence)
- `board` — kanban board updates (task move, create, delete, column changes)

## Key Patterns

- `SseManager` uses `tokio::sync::broadcast` internally — multiple consumers per entity
- `EventBuffer` replays missed events when client reconnects with `Last-Event-ID`
- Ring buffer evicts oldest events when full (500 cap per entity)
- Wire format maps `StreamEvent` variants to SSE event types (e.g., `content-delta`, `tool-use`, `message-stop`)
- Kanban wire events: `task-created`, `task-moved`, `task-updated`, `task-deleted`, `column-created`, `column-updated`, `column-deleted`

## Connections

- **Used by:** `api/mod.rs` (SSE endpoint handlers), `service/sessions.rs` (broadcasting agent output), `service/kanban.rs` (board events)
- **No dependencies** on other weave modules — pure infrastructure
