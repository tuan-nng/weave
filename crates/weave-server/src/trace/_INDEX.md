# trace/ — Trace Collection

Background trace event collection with async channel-based architecture. Single file: `mod.rs` (11KB, ~310 lines).

## Key Types

| Type | Purpose |
|------|---------|
| `TraceCollector` | Channel-based collector — `send(event)` pushes to unbounded mpsc channel; background task flushes batches to DB |
| `TraceEvent` | Event struct: `session_id`, `sequence` (auto-incremented), `kind`, `data` (JSON), `created_at` |

## Public API

- `TraceCollector::new(db, flush_interval)` — creates collector with background flush task
- `send(&self, session_id, kind, data)` — non-blocking push to channel
- `extract_file_changes(&self, session_id, events)` — parses tool-use events to build `FileChangeSummary` list (deduplicated by path, last write wins)

## Key Patterns

- Unbounded `mpsc` channel for trace events — non-blocking send, no backpressure
- Background flush task wakes every `flush_interval` and inserts pending events in a single transaction
- Event kinds: `ToolStart`, `ToolEnd`, `Decision`, `Error`, `FileWrite`, `FileRead`, `Message`
- File changes extracted from `FileWrite` events — deduplication uses HashMap keyed by path

## Connections

- **Used by:** `service/sessions.rs` (records agent decisions, tool calls, errors), `api/traces.rs` (read endpoints)
- **Depends on:** `store/traces.rs` (persistence), `db.rs` (connection pool)
