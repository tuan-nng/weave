# service/ — Business Logic Orchestration

Coordination layer between API handlers and store. Contains session lifecycle management, kanban automation, and cancellation infrastructure.

## Files

| File | Size | Contains |
|------|------|----------|
| `sessions.rs` | 99KB | `SessionService` + `ActiveSessions` (bounded concurrency tracker) — prompt lifecycle, SSE streaming, cancellation, session resume, agent execution. Also contains test agents: `CapturingAgent`, `SlowAgent`, `PartialAgent`, `ErroringAgent`, `EmptyAgent`, `MockStreamAgent`, `ThinkingAgent` |
| `kanban.rs` | 27KB | `try_automate_lane` (auto-triggered session creation when a task enters a column with specialist), `check_transition_gates` (enforces freeze/required-field/required-artifact policies before column moves) |
| `mod.rs` | 2KB | `ActiveSessions` — `HashMap<SessionId, CancellationToken>` with `try_insert`/`remove`/`contains`/`get` |

## Key Patterns

- `SessionService::start_prompt` is the main entry point — orchestrates agent call, streaming, tool execution loop, trace collection
- `SessionGuard` ensures stats tracking and cancellation cleanup happen on every code path
- `check_transition_gates` runs in a read transaction; `move_to_column` runs in a write transaction (minor TOCTOU window)
- `try_automate_lane` is called from API kanban task create/move — spawns a new session if column has auto_trigger + specialist

## Connections

- **Called by:** `api/sessions.rs`, `api/kanban.rs`
- **Calls:** `store/*` for persistence, `agent::ProviderRegistry` for agent creation, `tools::ToolRegistry` for tool execution, `sse::SseManager` for streaming, `trace::TraceCollector` for event logging
- **Key dependency:** `CancellationToken` from `tokio_util` for graceful cancellation
