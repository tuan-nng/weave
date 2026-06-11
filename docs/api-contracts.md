# API Contracts

## Common Envelope

All JSON responses use a consistent shape:

```json
// Success (single resource)
{"data": { ... }}

// Success (list)
{"data": [ ... ], "total": 42}

// Error
{"error": {"code": "not_found", "message": "Workspace not found"}}
```

**HTTP status codes:**
| Code | Meaning |
|------|---------|
| 200 | Success (GET, PATCH) |
| 201 | Created (POST) |
| 204 | No Content (DELETE) |
| 400 | Bad Request (validation failure) |
| 404 | Not Found |
| 409 | Conflict (duplicate, state conflict) |
| 500 | Internal Server Error |

## Workspace API

```json
// POST /api/workspaces
// Request:  {"name": "my-project"}
// Response (201): {"data": {"id": "uuid", "name": "my-project", "status": "active", "created_at": "...", "updated_at": "..."}}

// GET /api/workspaces
// Response (200): {"data": [...], "total": 1}

// GET /api/workspaces/:id
// Response (200): {"data": {"id": "uuid", "name": "my-project", "status": "active", ...}}

// PATCH /api/workspaces/:id
// Request:  {"name": "renamed-project"}
// Response (200): {"data": {...}}

// DELETE /api/workspaces/:id
// Response: 204 No Content
```

## Session API

```json
// POST /api/workspaces/:wid/sessions
// Request:
{
    "provider_id": "uuid",
    "specialist_id": "crafter",     // optional
    "model": "claude-sonnet-4-5",   // optional, overrides provider default
    "parent_session_id": "uuid"     // optional, for resume
}
// Response (201):
{"data": {"id": "uuid", "workspace_id": "wid", "provider_id": "uuid", "specialist_id": "crafter", "status": "connecting", ...}}

// POST /api/sessions/:sid/prompt
// Request:  {"message": "Implement the login page"}
// Response (200): {"data": {"message_id": "uuid"}}

// POST /api/sessions/:sid/cancel
// Response: 204 No Content

// GET /api/sessions/:sid/history?cursor=msg_abc123&limit=50
// cursor: optional, message ID from previous page
// limit: optional, default 50, max 200
// Response (200):
{
    "data": [
        {"id": "uuid", "role": "user", "content": "...", "created_at": "..."},
        {"id": "uuid", "role": "assistant", "content": "...", "created_at": "..."}
    ],
    "has_more": true,
    "next_cursor": "uuid"
}
```

## SSE Protocol

All SSE endpoints follow the same event format:

```
event: <event_type>
id: <sequential_id>
data: <json_payload>

```

**Session stream events** (`GET /api/sessions/:sid/stream`):

| Event Type | Payload | When |
|------------|---------|------|
| `connected` | `{"session_id": "..."}` | Client connected |
| `text_delta` | `{"text": "..."}` | Agent text chunk |
| `tool_use_start` | `{"id": "...", "name": "...", "input": {...}}` | Tool call begins |
| `tool_use_delta` | `{"id": "...", "delta": "..."}` | Tool input streaming |
| `tool_result` | `{"id": "...", "result": "..."}` | Tool call completes |
| `thinking` | `{"text": "..."}` | Agent reasoning |
| `done` | `{"stop_reason": "end_turn"}` | Generation complete |
| `error` | `{"message": "..."}` | Error occurred |
| `heartbeat` | `{}` | Keep-alive (every 15s) |

**Kanban stream events** (`GET /api/boards/:bid/stream`):

| Event Type | Payload | When |
|------------|---------|------|
| `task_created` | `{task}` | Card added |
| `task_moved` | `{"task_id": "...", "from_column": "...", "to_column": "..."}` | Card moved |
| `task_updated` | `{task}` | Card modified |
| `task_deleted` | `{"task_id": "..."}` | Card removed |
| `column_added` | `{column}` | Column added |
| `session_started` | `{"task_id": "...", "session_id": "..."}` | Lane automation triggered |
| `heartbeat` | `{}` | Keep-alive (every 15s) |

**Reconnection**: Clients send `Last-Event-ID: <id>` header. Server replays buffered events (last 100 per stream) after that ID.

## Provider API

`POST /api/providers` widens to a discriminated union on `kind`
(`"http"` default; `"cli"` for CLI runtimes pre-registered before the
per-adapter dispatch lands in feat-051).

```json
// POST /api/providers — kind=http (default)
{
    "kind": "http",
    "type": "anthropic",
    "name": "My Anthropic Provider",
    "base_url": "https://api.anthropic.com",
    "api_key": "sk-ant-...",
    "default_model": "claude-sonnet-4-5"
}
// Response (201):
{"data": {
    "id": "uuid",
    "type": "anthropic",
    "kind": "http",
    "name": "My Anthropic Provider",
    "default_model": "claude-sonnet-4-5",
    "binary_path": null,
    "args_json": null,
    "env_json": null,
    "permission_mode": null,
    "created_at": "..."
}}
// Note: api_key is never returned in responses (carried in the
// un-serialized config_json column only).

// POST /api/providers — kind=cli
{
    "kind": "cli",
    "type": "anthropic",
    "name": "My Claude Code",
    "default_model": "claude-sonnet-4-5",
    "binary_path": "/usr/local/bin/claude",
    "args_json": "[\"--verbose\"]",
    "env_json": "{\"LOG_LEVEL\":\"info\"}",
    "permission_mode": "accept-edits"
}
// Response (201): same shape as above, kind="cli", HTTP-only fields null.

// GET /api/providers/:id/models
// Response (200) for kind=http:
{"data": [
    {"id": "claude-sonnet-4-5", "name": "Claude Sonnet 4.5", "context_window": 200000},
    {"id": "claude-haiku-4-5", "name": "Claude Haiku 4.5", "context_window": 200000}
]}
// Response (501) for kind=cli: not yet dispatchable (lands in feat-042).
{"error": {"code": "not_implemented",
           "message": "CLI model list not available until feat-042"}}
```

The pre-existing wire shape used a nested `config: {...}` envelope. The
feat-039 shape is **flat** — `base_url`, `api_key`, `default_model`,
`binary_path`, etc. are top-level fields, and `config_json` is never
serialized. The pre-039 doc example above was inaccurate; this section
is the source of truth as of feat-039.

## Kanban API

```json
// POST /api/workspaces/:wid/boards
// Request:
{
    "name": "Sprint 1",
    "columns": [
        {"name": "Backlog", "specialist_id": "backlog-refiner", "auto_trigger": true},
        {"name": "To Do", "specialist_id": "todo-orchestrator", "auto_trigger": true},
        {"name": "In Progress", "specialist_id": "dev-crafter", "auto_trigger": true},
        {"name": "Review", "specialist_id": "review-guard", "auto_trigger": true},
        {"name": "Done", "specialist_id": "done-reporter", "auto_trigger": false}
    ]
}

// POST /api/boards/:bid/cards
// Request: {"column_id": "uuid", "title": "Implement login page", "description": "Build a login form with email/password validation"}

// PATCH /api/tasks/:tid
// Request (move task): {"column_id": "new-column-id", "position": 0}
// When moving to a column with auto_trigger=true and specialist_id set,
// the server automatically creates a session with that specialist.
```

## A2A Protocol (feat-029, feat-056)

Weave implements the A2A protocol v1.0 REST binding. A2A clients
discover capabilities via the Agent Card and exchange messages via
`POST /api/a2a/messages`.

### Agent Card — `GET /.well-known/agent.json`

Public (no auth required per the A2A spec). Returns the server's
capabilities and skills.

```json
{
  "name": "Weave",
  "description": "Multi-agent coordination platform ...",
  "url": "http://<host>",
  "version": "0.1.0",
  "defaultRuntimeKind": "anthropic-api",
  "capabilities": { "streaming": true, "pushNotifications": false },
  "skills": [
    { "id": "code-review", "name": "code-review", "description": "...", "tags": ["review"] }
  ],
  "defaultInputModes": ["text/plain"],
  "defaultOutputModes": ["text/plain"]
}
```

**`defaultRuntimeKind` (feat-056):** The runtime kind A2A
`SendMessage` will use when the request omits `runtimeKind` AND
there is no resuming session (`taskId`) to inherit one from. Wire
form is kebab-case, e.g. `"anthropic-api"`, `"claude-code"`. Sourced
from `state.a2a_default_runtime_kind` (set once at startup from
`WEAVE_A2A_DEFAULT_RUNTIME_KIND`; defaults to `anthropic-api`).
Clients SHOULD use this field as the basis for any "list available
runtimes" UI; the server does not expose a separate endpoint for
that today.

### SendMessage — `POST /api/a2a/messages`

Authenticated (Bearer token via `WEAVE_A2A_TOKEN` env var, when
configured). Creates a new session or continues an existing one
(via `taskId`).

```json
// Request
{
  "message": {
    "role": "user",
    "parts": [{ "type": "text", "text": "Hello, world." }]
  },
  "contextId": "ctx-123",          // optional
  "taskId": "session-abc",         // optional — set to resume an existing session
  "runtimeKind": "claude-code",    // optional — see Resolution order
  "mode": "wrapped"                // optional — see Resolution order
}

// Response 201 Created
{
  "data": {
    "id": "session-abc",
    "contextId": "ctx-123",
    "status": "submitted"
  }
}
```

### Runtime-kind resolution order (feat-056)

The server picks which `RuntimeKind` (and matching provider) an
A2A session lands on using this chain. **This is a breaking
change vs pre-feat-056: the silent "first provider in the table"
fallback is removed.**

1. **Body `runtimeKind`** (if present) — the explicit client
   override. Wire form: kebab-case
   (`"anthropic-api"`, `"openai-api"`, `"openai-compatible"`,
   `"claude-code"`, `"codex"`, `"opencode"`).
2. **Resuming session's stored `runtime_kind`** (when `taskId` is
   set and the request omits `runtimeKind`).
3. **`state.a2a_default_runtime_kind`** — the Agent Card's
   `defaultRuntimeKind`. Read once at startup from
   `WEAVE_A2A_DEFAULT_RUNTIME_KIND`; falls back to
   `RuntimeKind::default()` (`anthropic-api`) when unset, empty,
   or unparseable.

`mode` is resolved in the same way:

1. **Body `mode`** (if present) — explicit override.
2. **`supported_modes(runtime_kind)[0]`** — derived from the
   resolved runtime. HTTP runtimes → `"native"`; CLI runtimes →
   `"wrapped"`. The runtime × mode compatibility check is
   enforced inside `SessionService::create_session` (the
   feat-040 chokepoint); an incompatible pair is rejected with
   `code: "runtime_mode_incompatible"` BEFORE any session row
   is created.

### Provider selection

After the runtime is resolved, the server picks a provider from
`providers` whose `kind` matches (creation order), filtered by
`ProviderRegistry::cached_health_for` (the 10-second health cache
the provider wizard uses). Cold cache (no probe has run this
process lifetime) means every provider is treated as unhealthy —
a known limitation that pre-feat-056 also had, just hidden by the
silent first-row fallback.

When no healthy provider exists for the resolved runtime:

- **New session**: returns `400 Bad Request` with
  `code: "no_provider_for_runtime"` and a message naming the
  missing runtime.
- **Resume (`taskId`)**: the server logs a `tracing::warn!` and
  falls back to `a2a_default_runtime_kind`'s providers. Only if
  BOTH the prior runtime AND the env default have no healthy
  provider does the resume error with
  `code: "no_provider_for_runtime"`. The session row is not
  mutated; the next `send_prompt` attempt will dispatch on the
  session's stored runtime (which may still fail at the dispatch
  layer — that is graceful degradation, not a fix for missing
  providers).

### Error envelope (existing convention)

```json
{
  "error": {
    "code": "no_provider_for_runtime",
    "message": "no healthy AI provider configured for runtime 'claude-code'; add a provider via POST /api/providers, or set WEAVE_A2A_DEFAULT_RUNTIME_KIND to a runtime that has a provider"
  }
}
```

`code` values specific to A2A resolution:

| Code | When |
|------|------|
| `no_provider_for_runtime` | No healthy provider for the resolved runtime (and the resume fallback, if applicable) |
| `runtime_mode_incompatible` | `runtime_kind` × `mode` pair not in the compatibility matrix (see `agent::supported_modes`) |
| `cwd_outside_codebase` | `mode: "wrapped"` requires a cwd inside a registered codebase; the A2A handler does not supply one, so this is a hard reject (feat-050) |

