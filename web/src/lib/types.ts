// Domain models — match backend Rust structs exactly

export interface Workspace {
  id: string;
  name: string;
  status: string;
  is_default: boolean;
  created_at: string;
  updated_at: string;
}

export type SessionStatus = "connecting" | "ready" | "completed" | "error" | "cancelled";

export interface Session {
  id: string;
  workspace_id: string;
  provider_id: string;
  specialist_id: string | null;
  parent_session_id: string | null;
  status: SessionStatus;
  model: string | null;
  cwd: string | null;
  created_at: string;
  updated_at: string;
}

export interface Message {
  id: string;
  session_id: string;
  role: string;
  content: string;
  metadata: string | null;
  created_at: string;
}

export interface Provider {
  id: string;
  type: string;
  name: string;
  created_at: string;
}

export interface SpecialistInfo {
  name: string;
  description: string;
  model: string | null;
  tool_profile: string | null;
  tags: string[];
}

export interface Task {
  id: string;
  board_id: string;
  column_id: string;
  title: string;
  description: string | null;
  position: number;
  status: string;
  session_id: string | null;
  acceptance_criteria: string | null;
  completion_summary: string | null;
  verification_report: string | null;
  created_at: string;
  updated_at: string;
}

export interface TraceRow {
  id: string;
  session_id: string;
  event_type: string;
  summary: string;
  data_json: string | null;
  timestamp: string;
}

export interface FileChangeSummary {
  path: string;
  actions: string[];
  count: number;
}

export interface ModelInfo {
  id: string;
  name: string;
  context_window: number;
}

// ---------------------------------------------------------------------------
// Codebases (feat-032) — domain models matching `store::codebases` in
// `crates/weave-server/src/store/`. JSON field names mirror the Rust
// `Serialize` derives 1:1.
// ---------------------------------------------------------------------------

export interface Codebase {
  id: string;
  workspace_id: string;
  /// Absolute filesystem path.
  path: string;
  /// Optional branch hint (display-only — the codebase tracks the
  /// working tree, not a ref).
  branch: string | null;
  /// Optional human label (e.g. "Backend", "Mobile").
  label: string | null;
  created_at: string;
}

/// One recent commit (hash + first line of message).
export interface GitCommit {
  hash: string;
  message: string;
}

/// Git status snapshot. Returned inside `CodebaseDetail` when the
/// path is a git repo AND git is callable. `branch` is empty when
/// the repo has no commits. `dirty_files` is the union of
/// staged + unstaged + untracked paths. `recent_commits` is up to
/// the last 5 commits on HEAD.
export interface GitStatus {
  branch: string;
  dirty_files: string[];
  recent_commits: GitCommit[];
}

/// Composite response for `GET /api/codebases/{id}?wid={wid}`.
///
/// `git_status` is `null` and `git_error` is `Some(msg)` when the
/// path is not a git repo or git is broken. The row is always
/// present so the client can offer edit/delete regardless of git's
/// state.
export interface CodebaseDetail {
  codebase: Codebase;
  git_status: GitStatus | null;
  git_error: string | null;
}

// ---------------------------------------------------------------------------
// Kanban (feat-026) — domain models matching `store::boards`, `store::columns`,
// `store::tasks` in `crates/weave-server/src/store/`. JSON field names mirror
// the Rust `Serialize` derives 1:1.
// ---------------------------------------------------------------------------

export interface Board {
  id: string;
  workspace_id: string;
  name: string;
  created_at: string;
}

export interface Column {
  id: string;
  board_id: string;
  name: string;
  position: number;
  specialist_id: string | null;
  auto_trigger: boolean;
  created_at: string;
}

/// Returned by `GET /api/workspaces/{wid}/boards/{id}`. Flat shape — the
/// client groups `tasks` by `column_id` (the API does not pre-nest).
export interface BoardDetail {
  board: Board;
  columns: Column[];
  tasks: Task[];
}

export type TaskStatus = "active" | "done" | "archived";

/// One column spec used when creating a board with an inline template.
/// Mirrors `CreateColumnRequest` but the API accepts this shape on
/// `POST /api/workspaces/{wid}/boards` (see `api/kanban.rs:CreateBoardRequest`).
export interface NewColumnSpec {
  name: string;
  position?: number;
  specialist_id?: string;
  auto_trigger?: boolean;
}

// API envelopes

export interface ApiErrorResponse {
  error: {
    code: string;
    message: string;
  };
}

// Pagination

export interface PaginationParams {
  cursor?: string;
  limit?: number;
}

export interface PaginatedResponse<T> {
  data: T[];
  cursor?: string;
}

// Request bodies

export interface CreateWorkspaceRequest {
  name: string;
}

export interface UpdateWorkspaceRequest {
  name: string;
}

export interface CreateProviderRequest {
  type: string;
  name: string;
  base_url: string;
  api_key: string;
  default_model: string;
}

export interface CreateSessionRequest {
  provider_id: string;
  specialist_id?: string;
  model?: string;
  cwd?: string;
  parent_session_id?: string;
}

// ---------------------------------------------------------------------------
// Kanban request DTOs (feat-026). Tri-state semantics for nullable fields:
//   `undefined` (key absent) = "don't change"
//   `null`                  = "clear"
//   string value            = "set to this value"
// See `crates/weave-server/src/api/kanban.rs:47-119` for the source of truth.
// ---------------------------------------------------------------------------

export interface CreateBoardRequest {
  name: string;
  columns?: NewColumnSpec[];
}

export interface UpdateBoardRequest {
  name: string;
}

export interface CreateColumnRequest {
  name: string;
  position?: number;
  specialist_id?: string;
  auto_trigger?: boolean;
}

export interface UpdateColumnRequest {
  name?: string;
  position?: number;
  /// Tri-state: `undefined` = leave alone, `null` = clear, `string` = set.
  specialist_id?: string | null;
  auto_trigger?: boolean;
}

export interface CreateCardRequest {
  column_id: string;
  title: string;
  description?: string;
  position?: number;
  status?: TaskStatus;
}

// ---------------------------------------------------------------------------
// Codebase request DTOs (feat-032). Matches
// `crates/weave-server/src/api/codebases.rs:CreateCodebaseRequest`.
// ---------------------------------------------------------------------------

export interface CreateCodebaseRequest {
  /// Absolute filesystem path. The backend validates this is
  /// absolute, exists, and is a git repo at create time.
  path: string;
  branch?: string;
  label?: string;
}

export interface UpdateTaskRequest {
  title?: string;
  /// Tri-state: `undefined` = leave alone, `null` = clear, `string` = set.
  description?: string | null;
  /// Changing `column_id` routes through `TaskStore::move_to_column` and
  /// triggers a position rebalance. Sending `column_id` via this DTO is
  /// allowed; the API handler intercepts it before the regular update.
  column_id?: string;
  position?: number;
  status?: TaskStatus;
  session_id?: string | null;
  acceptance_criteria?: string | null;
  completion_summary?: string | null;
  verification_report?: string | null;
}

// SSE event types — match backend SseWireEvent

export type SseEventType =
  | "text_delta"
  | "tool_use_start"
  | "tool_use_delta"
  | "tool_result"
  | "thinking"
  | "done"
  | "error"
  | "message_persisted"
  | "connected"
  | "gap";

export interface SseTextDeltaEvent {
  type: "text_delta";
  text: string;
}

export interface SseToolUseStartEvent {
  type: "tool_use_start";
  id: string;
  name: string;
  input: unknown;
}

export interface SseToolUseDeltaEvent {
  type: "tool_use_delta";
  id: string;
  delta: string;
}

export interface SseToolResultEvent {
  type: "tool_result";
  id: string;
  result: string;
}

export interface SseThinkingEvent {
  type: "thinking";
  text: string;
}

export interface SseDoneEvent {
  type: "done";
  stop_reason: string;
}

export interface SseErrorEvent {
  type: "error";
  message: string;
}

/// Emitted by the backend exactly once per assistant turn, AFTER the
/// assistant message has been written to the database and BEFORE the
/// terminal `done` event. The frontend uses the `id` to swap the live
/// streaming bubble for the persisted one — no content-string dedup
/// needed. `id === ""` is the sentinel for "no message was persisted
/// this turn" (e.g. cancel before any text streamed).
export interface SseMessagePersistedEvent {
  type: "message_persisted";
  id: string;
  role: string;
  stop_reason: string | null;
  content: string;
  created_at: string;
}

export interface SseConnectedEvent {
  type: "connected";
  session_id: string;
}

export interface SseGapEvent {
  type: "gap";
  missed: number;
}

export type SseEvent =
  | SseTextDeltaEvent
  | SseToolUseStartEvent
  | SseToolUseDeltaEvent
  | SseToolResultEvent
  | SseThinkingEvent
  | SseDoneEvent
  | SseErrorEvent
  | SseMessagePersistedEvent
  | SseConnectedEvent
  | SseGapEvent;

// ---------------------------------------------------------------------------
// Board-scoped SSE events (feat-026). The backend broadcasts these on entity
// `board:{bid}` via `GET /api/boards/{bid}/stream`. Field names match
// `crates/weave-server/src/sse/mod.rs:225-247` exactly.
// ---------------------------------------------------------------------------

export interface SseBoardTaskCreatedEvent {
  type: "task_created";
  task: Task;
}

export interface SseBoardTaskMovedEvent {
  type: "task_moved";
  task: Task;
  from_column_id: string;
  to_column_id: string;
}

export interface SseBoardTaskUpdatedEvent {
  type: "task_updated";
  task: Task;
}

export interface SseBoardTaskDeletedEvent {
  type: "task_deleted";
  task_id: string;
  column_id: string;
}

export interface SseBoardColumnAddedEvent {
  type: "column_added";
  column: Column;
}

/// Emitted by `try_automate_lane` when a task is moved into a column with
/// `auto_trigger=true` and a bound `specialist_id`. The frontend patches
/// `task.session_id` so the agent indicator pill appears on the card.
export interface SseBoardSessionStartedEvent {
  type: "session_started";
  session_id: string;
  task_id: string;
  specialist_id: string;
  board_id: string;
}

export interface SseBoardHeartbeatEvent {
  type: "heartbeat";
}

/// `connected` from the board stream carries `session_id: ""` (board id is
/// in the URL, not the payload). Treated as a no-op lifecycle marker.
export interface SseBoardConnectedEvent {
  type: "connected";
  session_id: string;
}

/// Protocol error event — emitted once when the requested board id
/// does not exist (the server can't open a stream for a missing board).
/// Subsequent errors during a live stream are surfaced via `es.onerror`
/// and re-triggered as `EventSource` auto-reconnects.
export interface SseBoardErrorEvent {
  type: "error";
  message: string;
}

export type SseBoardEvent =
  | SseBoardTaskCreatedEvent
  | SseBoardTaskMovedEvent
  | SseBoardTaskUpdatedEvent
  | SseBoardTaskDeletedEvent
  | SseBoardColumnAddedEvent
  | SseBoardSessionStartedEvent
  | SseBoardHeartbeatEvent
  | SseBoardConnectedEvent
  | SseBoardErrorEvent;

/// Parsed shape of `messages.metadata` (TEXT NULL in the database).
/// Currently a single optional `stop_reason` tag, but the JSON shape
/// leaves room for future fields without a migration.
export interface MessageMetadata {
  stop_reason?: "cancelled" | "error" | "max_tokens";
}

// Health

export interface HealthResponse {
  status: string;
  version: string;
  uptime_seconds: number;
}
