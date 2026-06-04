# a2a/ — Google A2A Protocol v1.0

Server-side implementation of the Google Agent-to-Agent protocol. Exposes an Agent Card and task-oriented endpoints.

## Files

| File | Size | Contains |
|------|------|----------|
| `mod.rs` | 1KB | Router assembly: `a2a_routes(state)` — mounts all A2A endpoints under `/.well-known/` and `/a2a/` |
| `types.rs` | 9KB | Core types: `AgentCard`, `AgentCapabilities`, `AgentSkill`, `Task`, `TaskStatus`, `TaskState`, `Artifact`, `Message`, `Part`, `SendMessageRequest`, `GetTaskRequest`, `CancelTaskRequest`. All serde-tagged for JSON-RPC-ish protocol. |
| `agent_card.rs` | 2KB | `GET /.well-known/agent.json` — returns Agent Card describing weave's capabilities |
| `tasks.rs` | 11KB | Task endpoints: `SendMessage` (creates/queues a task), `GetTask` (polls status), `CancelTask` (cancels), `SubscribeToTask` (SSE stream of task events) |
| `messages.rs` | 5KB | Message handling — converts A2A messages to/from internal session messages |
| `auth.rs` | 3KB | Authentication helpers — token extraction and validation for A2A requests |

## Key Patterns

- Protocol version: A2A v1.0
- Endpoints: `POST /a2a/send`, `POST /a2a/task`, `POST /a2a/cancel`, `GET /a2a/task/:id/stream`
- Task state machine: `pending → working → completed/failed/canceled`
- Tasks map to internal kanban sessions — A2A tasks create sessions via `SessionService`
- Migration 009 added `context_id` to sessions table for A2A correlation

## Connections

- **Calls:** `service::SessionService` for task execution, `store::sessions` for persistence
- **Used by:** External agents via HTTP — not called internally
