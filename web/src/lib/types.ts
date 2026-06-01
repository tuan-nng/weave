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
