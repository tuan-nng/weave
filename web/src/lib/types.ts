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

/// feat-054: runtime / mode / resume-state enums. The wire forms mirror
/// the Rust `Serialize` derives 1:1 — `kebab-case` for `RuntimeKind`
/// (e.g. `"claude-code"`) and `snake_case` for `SessionMode` and
/// `ResumeState` (e.g. `"replayed"`). Kept as string-literal unions so
/// a typo at a call site is a compile error and the wire shape is
/// readable at the use site.
export type RuntimeKind =
  | "anthropic-api"
  | "openai-api"
  | "openai-compatible"
  | "claude-code"
  | "codex"
  | "opencode";

export type SessionMode = "native" | "wrapped" | "attended";

/// Per-turn resume outcome, broadcast on every `done` /
/// `message_persisted` SSE event (feat-047). The frontend renders it
/// as a pill in the session header so the user can see whether the
/// agent is continuing from a stored CLI session id, replaying from
/// history, or starting fresh.
export type ResumeState = "none" | "native" | "replayed";

export interface Session {
  id: string;
  workspace_id: string;
  provider_id: string;
  specialist_id: string | null;
  parent_session_id: string | null;
  status: SessionStatus;
  model: string | null;
  cwd: string | null;
  /// When set, the session is bound to a registered codebase. The
  /// session's `cwd` mirrors the codebase's path (the binding wins at
  /// create time, overriding any `cwd` arg). When the referenced
  /// codebase is deleted, this becomes `null` (`ON DELETE SET NULL`).
  codebase_id: string | null;
  /// feat-038+: which Runtime Tool the session is bound to. The page
  /// uses this (with `mode`) to pick the chat layout — see feat-054.
  runtime_kind: RuntimeKind;
  /// feat-038+: the session's interaction model. The page switches
  /// header / banner variants on this value. `attended` is reserved
  /// for Phase 11 and not yet accepted at create time.
  mode: SessionMode;
  /// feat-047: per-runtime JSON blob persisted on the row
  /// (e.g. `{cli_resume_id: "..."}` for CLI runtimes). `null` for
  /// HTTP runtimes and for CLI sessions that have not yet captured a
  /// stored id.
  runtime_metadata_json: string | null;
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
  kind: "http" | "cli";
  name: string;
  default_model: string | null;
  binary_path: string | null;
  args_json: string | null;
  env_json: string | null;
  permission_mode: string | null;
  /// feat-053: per-provider health snapshot from `ProviderRegistry`'s
  /// 10s `HealthCache`. `false` when the cache has never been warmed
  /// for this provider id (the conservative default — the wizard
  /// treats unseen-cache as unhealthy and grey-out non-selectable).
  /// The frontend doesn't probe on its own; the next health check
  /// tick will flip it.
  healthy: boolean;
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
  runtime_kind: RuntimeKind | null;
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
  runtime_kind?: RuntimeKind;
}

/// Per-column automation config (feat-066).
export interface DeliveryRules {
  require_committed_changes?: boolean;
  require_clean_worktree?: boolean;
}

export interface ContractRules {
  require_canonical_story?: boolean;
}

export interface ChecklistRules {
  required_checklist?: boolean;
}

export interface ValidatorCommand {
  command: string;
  /// Seconds before the subprocess is killed. Default 30.
  timeout_secs?: number;
}

export type GateMode = "blocking" | "warning";

export interface AutomationConfig {
  required_artifacts?: string[];
  delivery_rules?: DeliveryRules;
  contract_rules?: ContractRules;
  checklist_rules?: ChecklistRules;
  validator_command?: ValidatorCommand;
  gate_mode?: GateMode;
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
  // Default "http" when omitted. The Rust handler defaults missing `kind`
  // to "http" for back-compat with pre-feat-039 callers.
  kind?: "http" | "cli";
  type: string;
  name: string;
  // HTTP-only (required for kind=http, must NOT be set for kind=cli).
  base_url?: string;
  api_key?: string;
  // Common to both kinds.
  default_model?: string;
  // CLI-only (required for kind=cli, must NOT be set for kind=http).
  binary_path?: string;
  args_json?: string;
  env_json?: string;
  permission_mode?: string;
}

/// feat-053: matches the backend's `CreateSessionRequest` after the
/// runtime/mode widening in feat-038. The legacy `cwd` field is gone
/// (use `codebase_id` instead — the server copies the codebase's path
/// onto the session); `runtime_kind` / `mode` / `runtime_metadata_json`
/// are the new optional fields. The kebab-case strings map 1:1 to the
/// backend's `RuntimeKind` / `SessionMode` enums; unknown values get
/// rejected with 400 at parse time, so we don't need to mirror the
/// Rust enums in TS.
export interface CreateSessionRequest {
  provider_id: string;
  specialist_id?: string;
  model?: string;
  codebase_id?: string;
  parent_session_id?: string;
  /// One of the values the backend accepts in `RuntimeKind`:
  /// `"anthropic-api"`, `"openai-api"`, `"openai-compatible"`,
  /// `"claude-code"`, `"codex"`, `"opencode"`. The wizard picks
  /// this from the chosen provider's `kind` (`http` →
  /// `"anthropic-api"`, `cli` → `"claude-code"`); the type is the
  /// union of wire strings for clarity.
  runtime_kind?: RuntimeKind;
  /// One of the values the backend accepts in `SessionMode`:
  /// `"native"`, `"wrapped"`. (`"attended"` is reserved for Phase
  /// 11 and rejected at create time.) The wizard pairs this with
  /// `runtime_kind` per the runtime×mode matrix (see
  /// `web/src/lib/runtime-matrix.ts`).
  mode?: "native" | "wrapped";
  /// Per-runtime JSON blob. Most call sites leave this `undefined`;
  /// the wizard only forwards it on resume flows. Shape is keyed
  /// on `runtime_kind` (e.g. `{cli_resume_id: "..."}` for CLI).
  runtime_metadata_json?: string | null;
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
  runtime_kind?: RuntimeKind;
  /// Per-column automation config (delivery/contract/checklist/validator
  /// gates + gate_mode). Optional; omit = no automation (legacy).
  automation?: AutomationConfig;
}

export interface UpdateColumnRequest {
  name?: string;
  position?: number;
  /// Tri-state: `undefined` = leave alone, `null` = clear, `string` = set.
  specialist_id?: string | null;
  auto_trigger?: boolean;
  /// Tri-state: `undefined` = leave alone, `null` = clear, `string` = set.
  runtime_kind?: RuntimeKind | null;
  /// Automation config. Tri-state like runtime_kind above.
  automation?: AutomationConfig | null;
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
  /// feat-047 / feat-054: per-turn runtime metadata. The frontend
  /// stores these on the live buffer (last value wins) so the header
  /// pill row can render them on the next paint without an extra
  /// round-trip. `runtime_metadata_json` is `null` for HTTP runtimes
  /// and for CLI sessions that have not yet captured a stored id.
  runtime_kind: RuntimeKind;
  mode: SessionMode;
  resume_state: ResumeState;
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
  /// feat-047 / feat-054: per-turn runtime metadata mirrored on
  /// `message_persisted` (emitted right before the terminal `done`
  /// event). The frontend folds these into the live buffer so the
  /// header pill row is consistent from the moment the persisted
  /// message appears — see `use-session.ts:handleEvent`.
  runtime_kind: RuntimeKind;
  mode: SessionMode;
  resume_state: ResumeState;
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
