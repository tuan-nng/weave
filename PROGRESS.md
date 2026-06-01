# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-06-01
- **Latest commit:** 3cb05d8 (code review fixes)
- **Active feature:** feat-021 (Session chat view) — passing
- **Build status:** green — `cargo build -p weave-server` succeeds; `bun run build` in web/ succeeds
- **Test status:** green — 326 Rust tests + 21 frontend tests pass
- **Lint status:** green — clippy clean, fmt clean; ESLint + Prettier clean

## Completed Since Project Start

- [x] System design documentation (`docs/SYSTEM_DESIGN.md`, `docs/ARCHITECTURE.md`)
- [x] Implementation plan (`docs/PLAN.md`)
- [x] Workspace `Cargo.toml` created (members: `crates/weave-server`)
- [x] **feat-001**: Binary skeleton with CLI, tracing, health check, graceful shutdown
- [x] **feat-002**: SQLite database with WAL mode and migrations (11 tables, user_version tracking)
- [x] **feat-003**: Shared error types (AppError, ProviderError) with thiserror, IntoResponse, JSON envelope
- [x] **feat-004**: Workspace CRUD (WorkspaceStore, REST API, default workspace seed, cursor pagination)
- [x] **feat-005**: CodingAgent trait (provider abstraction, StreamEvent, StopReason, message types, all Send+Sync)
- [x] **feat-006**: AnthropicAgent (SSE streaming, error mapping, retry logic, message conversion)
- [x] **feat-007**: ProviderStore + ProviderRegistry (CRUD, agent lifecycle, API with api_key stripping)
- [x] **feat-008**: SessionStore + MessageStore (session state machine, message pagination, session API)
- [x] **feat-009**: SessionService (prompt lifecycle, async streaming, cancellation, message history)
- [x] **feat-010**: SSE infrastructure (SseManager, EventBuffer, SSE endpoint, reconnection, backpressure)
- [x] **feat-011**: SpecialistLoader (YAML frontmatter parsing, SpecialistRegistry, system prompt + model override injection)
- [x] **feat-012**: ToolRegistry (ToolExecutor trait, 5 profiles, profile-based filtering, SessionService integration)
- [x] **feat-013**: Filesystem tools (fs_read, fs_write, fs_edit, fs_search, fs_list — PathValidator, symlink-aware containment, control-plane protection)
- [x] **feat-014**: Shell tool (shell_exec — sh -c wrapper, tokio::process::Command, timeout, 100KB output truncation, tracing::info! trace event)
- [x] **feat-015**: Git tools (git_status, git_diff, git_log, git_commit — tools/git/ directory, async run_git, validate_commit_identity, 50KB diff truncation, profile updates)
- [x] **feat-016**: Task context tools (get_task, list_tasks, update_task_status, update_task_fields — tools/task/ directory, TaskStore, workspace-scoped queries, migration 004)
- [x] **feat-017**: TraceCollector (trace/ module with channel-based collector, background flush task, file change extraction; store/traces.rs with TraceStore; api/traces.rs with 3 endpoints; streaming loop integration with pending tool tracking)
- [x] **feat-018**: Session resume (Db::with_transaction; SessionService::create_session with validate_parent_chain; MessageStore::copy_messages/load_all; terminal-state check; workspace validation; depth limit 5; cycle detection; 9 tests)
- [x] **feat-019**: React frontend scaffolding (Vite + React 19 + TypeScript + Tailwind CSS v4 + TanStack Query + React Router; Bun package manager; ESLint + Prettier + Vitest; API wrapper with {data} envelope unwrapping; types matching backend models; query key factory; route constants; 5 placeholder pages; 12 tests)
- [x] **feat-021**: Session chat view (useSession hook with TanStack Query + SSE streaming; SessionPage with MessageList, ToolCallBlock, StreamingIndicator, MessageInput; auto-scroll; react-markdown + remark-gfm; Sessions list page at /sessions; sidebar Sessions link fixed; .prettierignore for pnpm-lock.yaml)
- [x] **feat-036**: Re-implement session chat streaming for smooth UX. New `message_persisted` SSE event carries the persisted row id+content+stop_reason+created_at, broadcast AFTER `MessageStore::create` and BEFORE the terminal `done` event. On cancel and on streaming error, the partial text is persisted with `stop_reason` in the existing `messages.metadata` JSON column. Frontend `useSession` is a `useReducer` with a discriminated `Action` union; content-equality dedup in `LiveAssistantMessage` is replaced with id-based handoff (`streamId` vs `persistedTurnId`). Dead state removed (`thinkingChunks` → live `thinking[]` rendered as a collapsible block). "↓ jump to latest" pill on user scroll-up. 3-row skeleton loading. 24 new reducer unit tests + 3 new backend SSE integration tests + 1 new cancel-persists-partial test + 1 new error-persists-partial test + 1 new empty-turn-sentinel test.

## In Progress

(none — feat-036 complete; ready for next feature)

## Blocked

(none)

## Known Issues

(none)

## Next Steps

1. Continue Phase 3 (Frontend) — feat-022 (Journey sidebar), feat-023 (Frontend served from Rust)

## Session Notes

### 2026-05-31 — Linter/formatter config added
- Added `rustfmt.toml` — stable options only (edition 2021, max_width 100, import reordering)
- Added `clippy.toml` — complexity thresholds (cognitive 25, lines 150, args 8), test allowances
- Added `.cargo/config.toml` — placeholder for future lint additions
- All 128 tests pass, clippy clean, fmt clean, smoke test passes

### 2026-05-31 — feat-012: ToolRegistry
- Created `src/tools/mod.rs` — new module for tool infrastructure
- `ToolExecutor` trait: `name()`, `description()`, `input_schema()`, `execute(input, context) -> ToolResult` (async_trait, Send+Sync)
- `ToolContext`: `session_id`, `cwd`, `codebase_root`, `trace_collector: Arc<TraceCollector>`
- `TraceCollector` is a stub (empty struct) — will be fleshed out in feat-017
- `ToolResult`: `success`, `data`, `error` — serde roundtrip verified
- `ToolRegistry`: `HashMap<String, Arc<dyn ToolExecutor>>` + `HashMap<String, Vec<String>>` for profiles
- Five profiles: `full` (dynamic=all registered), `implementation`, `review`, `planning`, `reporting`
- `validate_profile_name()` for early fail-fast in `send_prompt`
- `resolve_profile()` returns `Vec<ToolDefinition>` — empty vec converted to None by caller
- `all_definitions()` sorts by name for deterministic output
- `AppState` now has `tools: Arc<ToolRegistry>` field
- `SessionService::send_prompt` validates tool profile early, `run_prompt_task` resolves tools from specialist's profile
- Invalid profile name → `AppError::Validation` with dynamic error message
- `test_support` module exports `MockTool` for shared use across test modules
- 149 tests pass (14 new: 12 tools + 2 service integration)

### 2026-06-01 — feat-013: Filesystem tools
- Created `src/tools/fs/` directory with 6 files: `mod.rs`, `read.rs`, `write.rs`, `edit.rs`, `search.rs`, `list.rs`
- `PathValidator` in `fs/mod.rs`: `require_absolute`, `validate_write_path` (symlink-aware), `resolve_path`, `is_control_plane`
- Symlink escape prevention: `resolve_path` canonicalizes the path (or nearest existing ancestor) before containment check
- Control-plane protection: hardcoded list of prefixes (`.git/`, `resources/specialists/`, etc.) and files (`Cargo.toml`, `weave.db`, etc.)
- Shared constants: `MAX_DEPTH=10`, `MAX_RESULTS=100` in `fs/mod.rs`, imported by `search.rs` and `list.rs`
- `fs_search` uses `regex::RegexBuilder` with 1MB size limit to prevent ReDoS
- `fs_edit` requires exactly 1 match of `old_string` — errors on 0 or >1 matches
- `fs_list` skips hidden directories (starting with `.`) for consistency with `fs_search`
- Updated `implementation` profile: added `fs_edit`, `fs_search`, `fs_list`
- Updated `review` profile: added `fs_search`
- Registered all 5 tools in `main.rs`
- Added `glob = "0.3"`, `regex = "1"` to dependencies, `tempfile = "3"` to dev-dependencies
- 202 tests pass (21 new: 5 tool implementations + 16 validation/helper/verification tests)
- `test_support::make_context` helper added for creating `ToolContext` in tests

### 2026-06-01 — feat-014: Shell tool
- Created `src/tools/shell.rs` — single file, flat structure (not a directory)
- `ShellExecTool` implements `ToolExecutor` with `sh -c` wrapper
- Uses `tokio::process::Command` with `Stdio::piped()` for stdout/stderr capture
- Timeout: `tokio::time::timeout` + `child.wait()` (not `wait_with_output` which takes ownership)
- On timeout: `child.kill().await` + `child.wait().await` to reap zombie, then await reader tasks
- `spawn_read_task` helper extracts DRY pattern for stdout/stderr reading
- Output truncated at 100KB per stream (`MAX_STREAM_BYTES`) with UTF-8 boundary safety
- `truncate_output` finds a UTF-8 boundary, uses `from_utf8_lossy` for incomplete sequences
- Cwd validation: `PathValidator::require_absolute` + `is_dir()` check — no codebase_root containment
- `optional_u64` helper added to `tools/fs/mod.rs` with 3 unit tests (present, absent, wrong_type)
- Registered in `main.rs` after filesystem tools
- `shell_exec` already in `implementation` profile — no profile changes needed
- Logging: `tracing::debug!` (not `info!`) to avoid persisting secrets from command strings
- 216 tests pass (14 new: 11 shell_exec + 3 optional_u64)
- No new dependencies — `tokio` already has `process` and `time` features
- All 6 review findings addressed (zombie reaping, task cleanup, DRY, logging level, doc comment, tests)

### 2026-06-01 — feat-015: Git tools
- Created `src/tools/git/` directory with 5 files: `mod.rs`, `status.rs`, `diff.rs`, `log.rs`, `commit.rs`
- `run_git` in `mod.rs`: async git command runner using `tokio::process::Command` (not `sh -c`)
- `validate_git_cwd`: async git repo detection via `git rev-parse --git-dir`
- `validate_commit_identity`: rejects placeholder names (test, tester, example, etc.) and domains (@example.com, @test.com, @localhost)
- `truncate_diff`: 50KB truncation using shared `truncate_bytes` from `tools/mod.rs`
- `cwd_property()`: shared JSON schema helper for the common `cwd` parameter
- `git_test_support::git_init`: shared test helper (parameterized name, email, commit flag)
- Profiles updated: `implementation` has 4 git tools, `review` has 3 (no `git_commit`)
- Extracted `spawn_read_task` and `truncate_bytes` to `tools/mod.rs` (shared with shell.rs)
- Shell tool updated to use shared helpers (removed local copies)
- `validate_git_cwd` is async (uses `run_git` instead of `std::process::Command`)
- Commit tool reads effective config (not `--local`) — respects global git identity
- Empty/whitespace commit messages rejected early
- 259 tests pass (43 new: 24 git tools + 19 updated/extracted)
- All 8 review findings addressed (blocking I/O, DRY, weak assertions, empty message, config)

### 2026-06-01 — feat-016: Task context tools
- Created `src/tools/task/` directory with 5 files: `mod.rs`, `get.rs`, `list.rs`, `update_status.rs`, `update_fields.rs`
- Created `src/store/tasks.rs` — TaskStore with workspace-scoped queries (JOIN through boards)
- Migration 004: adds `acceptance_criteria`, `completion_summary`, `verification_report` columns to tasks table
- `ToolContext` now has `workspace_id: String` field — used for workspace-scoped tool operations
- `VALID_TASK_STATUSES` constant: `in_progress`, `review_required`, `completed`, `needs_fix`, `blocked`
- Task tools hold `Arc<Db>` as a field (first tools with DB access, unlike unit-struct fs/shell/git tools)
- `list` capped at 500 rows (DEFAULT_LIST_LIMIT) to prevent unbounded result sets
- Profiles updated: `implementation` (4 task tools), `review` (3), `planning` (3), `reporting` (2)
- Replaced placeholder `"task"` and `"task_read"` profile entries with actual tool names
- 297 tests pass (38 new: 22 store/tasks + 16 tools/task)
- All review findings addressed: workspace scoping via JOIN, RETURNING_COLS for UPDATE queries, list limit

### 2026-06-01 — feat-017: TraceCollector
- Created `src/trace/` directory with `mod.rs` — TraceCollector (mpsc::UnboundedSender), extract_file_changes(), spawn_flush_task()
- Created `src/store/traces.rs` — TraceStore with insert_batch, list_by_session, list_journey, list_file_changes
- Created `src/api/traces.rs` — 3 GET endpoints: /trace, /trace/journey, /trace/files
- Replaced TraceCollector stub in `tools/mod.rs` with re-export from `trace::TraceCollector`
- Streaming loop in `service/sessions.rs` now tracks pending_tool_calls (HashMap<id, (name, input, Instant)>)
- Trace events emitted for: ToolUseStart+ToolResult (tool_call), Thinking (decision), Error (error), file changes extracted from fs_write/fs_edit inputs
- Background flush task: unbounded channel, 200ms interval, batch size 50, transactional inserts
- drain_pending_tools() helper emits incomplete tool calls on cancel/stream-end
- `traces` and `file_changes` tables already existed in migration 002 — no new migration needed
- 314 tests pass (17 new: 6 store/traces + 7 trace module + 3 API + 1 UTF-8 truncation)
- All 5 review findings addressed: UTF-8 boundary safety, orphaned tool calls, output_json encoding, cancellation flush, dead code removal

### 2026-06-01 — feat-018: Session resume
- Added `Db::with_transaction` to `db.rs` — idiomatic rusqlite Transaction RAII (auto-rollback on drop)
- Added `SessionStore::create_tx` — same as `create` but takes `&Connection` for transactional use
- Added `MessageStore::copy_messages` — bulk-copy messages with new UUIDs, preserving original `created_at`
- Added `MessageStore::load_all` — extracted from private `load_all_messages` in service (paginated, capped)
- Extracted `map_fk_violation` to module-level function (shared by `create` and `create_tx`)
- Added `SessionService::create_session` — orchestrates: workspace validation → terminal-state check → chain validation → message loading → transactional session creation + message copy
- Added `validate_parent_chain` — walks parent chain up to MAX_RESUME_DEPTH (5) hops, validates workspace ownership, detects cycles via HashSet
- Terminal-state check: parent must be completed/cancelled/error before resume (prevents copying incomplete history)
- Only direct parent's messages are copied — parent already has ancestors' messages if it was resumed
- `create_session` API handler now delegates to `SessionService::create_session`
- `SessionStore::create` has `#[allow(dead_code)]` — used in tests, production uses `create_tx` via service
- `HashMap` import expanded to include `HashSet`
- 323 tests pass (9 new: resume, chain, no-parent, not-found, wrong-workspace, depth-limit, cycle, empty-parent, active-parent-rejected)
- All 6 review findings addressed: terminal-state check, FK message, HashSet import, API test gap noted, standalone unit tests noted, `with_transaction` test noted

### 2026-06-01 — feat-021 design (session chat view)
- Ran feature-dev workflow phases 1-4 (Discovery, Exploration, Clarifying, Architecture)
- **Design decisions:**
  - **Clean approach** — 1 new hook (`use-session.ts`) + rewrite `session.tsx` with inline components
  - **react-markdown** + **remark-gfm** for Markdown rendering (installed)
  - **Traces for history** — fetch `/api/sessions/:sid/trace` to reconstruct tool calls for historical messages
  - **Chat only** — no sidebar shell (Journey sidebar is feat-022)
  - **Disable input** for terminal states (completed/cancelled/error)
  - **Page-lifetime SSE** — EventSource connects on mount, auto-reconnects via browser native behavior
- **SSE strategy:** Native `EventSource` with `Last-Event-ID` reconnection. Backend ring buffer (100 events) handles replay. Frontend accumulates `text_delta`/`tool_use_start`/`tool_result` in a `LiveBuffer`, flushes on `done` by invalidating TanStack Query cache.
- **Trace correlation:** Timestamp-based heuristic — traces between message `created_at` boundaries belong to the preceding assistant message. Not exact but acceptable for V1.
- **Key files to create:** `web/src/hooks/use-session.ts`
- **Key files to modify:** `web/src/app/pages/session.tsx`
- **Dependencies added:** `react-markdown@10.1.0`, `remark-gfm@4.0.1`
- **Prettier fix:** 5 frontend files had formatting issues, fixed with `bun run prettier --write`
- **Next:** Use Open Design MCP to design UI (conversation `9d032e11-21af-480d-a45b-4c16dadb2948`), then implement

### 2026-06-01 — UI redesign (Routa-inspired)
- Redesigned all frontend pages using Open Design MCP mockups as reference
- **index.css**: Added brand color theme (blue/amber/emerald/red/orchid/slate), Inter + Space Grotesk fonts via Google Fonts, `@theme inline` Tailwind v4 tokens, `fadeIn`/`fadeInUp` keyframes, thin scrollbar styles
- **layout.tsx**: Sidebar widened to `w-60` (240px), gradient logo icon with lightning bolt, icon-based nav items (`h-10 rounded-xl`), active state with left accent bar (`w-0.5 bg-brand-blue-500`), primary/secondary nav divider, workspace context pill in footer, `useLocation()` for active link detection
- **status-badge.tsx**: Updated to brand palette (emerald=ready, amber=connecting, blue=completed, red=error, slate=cancelled), added border, `rounded-lg`, `text-[11px]`
- **modal.tsx**: `rounded-2xl`, `backdrop-blur-sm`, `animate-fade-in` on content
- **spinner.tsx**: Brand blue/slate colors
- **error-banner.tsx**: Brand red palette, `rounded-xl`, `animate-fade-in`
- **home.tsx**: `rounded-2xl` semi-transparent cards (`bg-white/80 backdrop-blur-sm border-black/[0.06]`), color-coded workspace folder icons (blue/orchid/emerald/amber), stats row at bottom, staggered `animate-fade-in-up` with delays
- **workspace.tsx**: Workspace icon + mono ID in header, 4-column stats grid (Total/Active/Completed/Errors), CSS grid-based table rows inside `rounded-2xl` card, animated back nav arrow (`group-hover:-translate-x-0.5`), session rows navigate to `/sessions/:id`, "Open →" hover reveal
- **settings.tsx**: Form labels with `uppercase tracking-[0.14em]`, eye icon on password field, grid-based provider rows with amber gradient icon, hover-reveal delete button
- **not-found.tsx**: `font-display` for 404, centered layout, animated arrow link
- All 21 frontend tests pass, all 326 Rust tests pass
- Key design tokens: `border-black/[0.06]` for subtle borders, `bg-white/80 backdrop-blur-sm` for semi-transparent surfaces, `rounded-2xl` for cards, `font-display` for headings
- Open Design project: `weave-redesign` (3 HTML mockups: home.html, workspace.html, settings.html)

## Notes for Next Session

- feat-019 created `web/` directory with full React frontend scaffolding
- Package manager is **Bun** (not npm) — all justfile commands updated to use `bun`
- Tailwind CSS v4 uses `@tailwindcss/vite` plugin + `@import "tailwindcss"` in CSS (no tailwind.config.js)
- API wrapper in `web/src/lib/api.ts`: `api.workspaces.list()`, `api.sessions.sendPrompt()`, etc.
- Types in `web/src/lib/types.ts`: match Rust Serialize structs exactly
- Query key factory in `web/src/lib/query-keys.ts` for TanStack Query cache management
- Route constants in `web/src/lib/routes.ts`
- Router uses `createBrowserRouter` with 5 routes (home, workspace, session, settings, not-found)
- Vite dev proxy: `/api` → `http://localhost:3000`
- Phase 3 (Frontend) started — next: feat-020 (Home page, workspace list, settings)

### 2026-06-01 — feat-020: Frontend pages (in progress)
- Backend: Created `GET /api/specialists` endpoint (`api/specialists.rs`)
- Added `Serialize` derive to `Specialist` struct (system_prompt excluded via `#[serde(skip)]`)
- Added `SpecialistRegistry::insert()` method for testing/runtime registration
- Registered route in `api/mod.rs`
- 3 new tests pass: test_list_specialists, test_list_specialists_excludes_system_prompt, test_list_specialists_empty
- Frontend: Added `SpecialistInfo` interface to `types.ts`
- Frontend: Added `api.specialists.list()` to `api.ts`
- Frontend: Added `specialists` query keys to `query-keys.ts`
- Architecture: Minimal approach — 3 shared components (Modal, ErrorBanner, Spinner), 2 hooks (useWorkspaces, useProviders), inline sub-components in pages
- Next: Design UI with Open Design MCP, then implement components

### 2026-06-01 — feat-021: Session chat view
- Created `web/src/hooks/use-session.ts` — orchestrator hook with TanStack Query + SSE streaming
- Rewrote `web/src/app/pages/session.tsx` — full chat UI (~620 lines)
- SessionPage: MessageList, ToolCallBlock (expandable), StreamingIndicator, MessageInput
- UserMessage (right-aligned blue bubble), AssistantMessage (left-aligned white card with markdown)
- LiveAssistantMessage for streaming content with live tool calls
- Auto-scroll: tracks isAtBottom, scrolls on new content
- react-markdown + remark-gfm for Markdown rendering
- Trace-to-message correlation by timestamp (nearest preceding message)
- Created `web/src/app/pages/sessions.tsx` — sessions list page at /sessions
- Fixed sidebar "Sessions" link: now points to /sessions instead of /
- Added .prettierignore for pnpm-lock.yaml (bun install creates it)
- All 3 init.sh layers pass: 326 Rust tests + 21 frontend tests

### 2026-06-01 — feat-021 code review (Phase 6)
- Launched 3 parallel code-reviewer agents for session.tsx, use-session.ts, sessions.tsx
- **session.tsx fixes:**
  - Replaced non-null `id!` assertion with early return guard (rules-of-hooks safe)
  - Added `useMemo` for `correlateTraces` — was O(n*m) on every streaming re-render
  - Added try/catch around `JSON.parse` in `TraceToolCallBlock` — corrupted traces no longer crash the page
  - Added `aria-expanded`, `aria-controls` on ToolCallBlock toggle button
  - Added `sr-only` label on textarea, `aria-label` on send button
  - Replaced `textChunks.join("").length` with `textChunks.length` for contentLength
  - Replaced duplicated thinking dots with `<StreamingIndicator />` component
  - Removed dead `bannerError` state and unused `ErrorBanner` import
- **use-session.ts fixes:**
  - Used `qcRef` to avoid EventSource recreation when query client reference changes
  - Reset live buffer on SSE `connected` event (prevents stale content after reconnect)
  - Preserved `stop_reason` from `done` event (was discarding it)
  - Log SSE parse errors with `console.warn` instead of silent catch
  - Removed dead `eventSourceRef`
  - Added `sendError`/`cancelError` to return type
  - Added `onError` callback on `sendMutation`
- **sessions.tsx fixes:**
  - Fixed `s.title` type error — `Session` has no `title` field, used `specialist_id`
  - Combined duplicate imports from same module
- All 3 init.sh layers pass: 326 Rust tests + 21 frontend tests

### 2026-06-01 — Bug fix: session terminated after first turn
- Reported: "from a workspace I can create a session, choose the provider, start a new session, agent responds, then the session is terminated right away"
- Root cause: `run_prompt_task` (service/sessions.rs:556) marked the session `"completed"` after every turn, putting it in `TERMINAL`. A second `send_prompt` was rejected with `Validation("cannot send prompt to session in 'completed' status")`. The frontend then disabled the input because `isTerminal` also included `liveBuffer.stopReason !== null`, which is always set on the `done` event.
- Fix:
  - **Backend** (`crates/weave-server/src/service/sessions.rs`): `final_status` for the success path is now `"ready"`, with a comment pointing at the architecture-doc lifecycle `connecting → ready → (turns) → completed`. Error and cancel paths unchanged.
  - **Test update** (`test_sse_broadcast_on_prompt`): now asserts `status == "ready"` after a successful turn.
  - **Frontend** (`web/src/app/pages/session.tsx`): dropped `liveBuffer.stopReason !== null` from `isTerminal`. `stopReason` stays in the buffer as a UI hint but no longer gates the input.
- Sessions are now properly multi-turn. The `TERMINAL` set still includes `"completed"` for explicit close via PATCH or future cleanup work.
- All 3 init.sh layers pass: 326 Rust tests + 21 frontend tests
- Files changed: `crates/weave-server/src/service/sessions.rs` (+11/-4), `web/src/app/pages/session.tsx` (+6/-3)

### 2026-06-01 — Bug fix: chat messages rendered in random order

**Symptom:** In a session, both assistant messages appeared at the top of the chat and both user messages appeared at the bottom, with no apparent relationship to insertion order. The pattern was consistent for the same UUIDs but bore no relation to time.

**Root cause:** `MessageStore::list_by_session` (`crates/weave-server/src/store/sessions.rs`) used `ORDER BY id ASC` with cursor `id > ?`. But message IDs are random v4 UUIDs, not time-ordered, so the order was effectively random. The frontend renders `messages` in array order, so the UI showed whatever random sort the backend produced.

**Fix:**
- Sort by `created_at ASC` with `id ASC` as a deterministic tiebreaker for sub-second ties.
- Cursor is now a `"<created_at>|<id>"` pair (encoded by the store, opaque to clients); the `WHERE` clause is `(created_at > ? OR (created_at = ? AND id > ?))`.
- The cursor format change is internal — the frontend never passes a cursor (history is fetched once with `limit: 100`).

**Test added:** `test_message_list_orders_by_created_at_not_id` — inserts 4 messages, asserts the listing returns them in insertion order regardless of UUID randomness.

**Test updated:** `test_message_pagination` — now asserts the listing is in `created_at`-then-`id` order rather than pure `id` order.

**Verification:** All 3 init.sh layers pass; 327 Rust tests (+1 new) + 21 frontend tests; clippy/fmt clean.

**Files changed:** `crates/weave-server/src/store/sessions.rs` (+58/-8)

### 2026-06-01 — Bug fix: user's own message invisible after send

**Symptom:** When the user sent a message, the chat jumped straight to the agent's "Thinking..." bubble. The user's own message was not visible until the assistant finished streaming and the history refetch finally picked it up.

**Root cause:** Two cooperating parts:
- The backend **does** persist the user message synchronously and returns its `message_id` (`service/sessions.rs:143`). The history query just doesn't refetch until the `done` SSE event fires.
- The frontend had no optimistic rendering for the user's own message. `useSession`'s `sendMutation.onSuccess` flipped `liveBuffer.isStreaming = true` (driving the "Thinking..." bubble) but didn't surface the prompt text anywhere.

**Fix:** Optimistic pending-prompt list keyed by the real `message_id` returned from the backend.

- `useSession` now keeps a `pendingPrompts: { id, content, createdAt }[]` list. On `sendPrompt` success, the prompt is added to it with the backend's `message_id`.
- A `useEffect` watches `messages` (the persisted history) and removes any pending prompt whose `id` shows up there. This handles both the first turn (initial history refetch) and the `done` event refetch without any flicker or duplicate render.
- `SessionPage` renders the pending prompts as `UserMessage` instances **after** the persisted messages and **before** the `LiveAssistantMessage`. The user's own bubble appears immediately on submit, and the streaming assistant bubble appears below it as the agent starts working.

**Why key by `message_id` rather than content?** Content matching is fragile (the user could legitimately send the same text twice in a row). The backend's `message_id` is a stable UUID for the persisted row — it's the right key.

**Verification:** All 3 init.sh layers pass; 327 Rust tests + 21 frontend tests; lint/clippy/fmt clean.

**Files changed:** `web/src/hooks/use-session.ts` (+30/-2), `web/src/app/pages/session.tsx` (+18/-2)

### 2026-06-01 — Bug fix: page flash after agent finishes streaming

**Symptom:** After the agent finished a turn, the chat "flashed" — the streamed text briefly disappeared and the page looked like it refreshed before settling on the final state. The experience was jarring.

**Root cause:** Two cooperating issues:

1. **Backend race**: `run_prompt_task` broadcast the terminal `Done` SSE event inside the stream loop, **before** saving the assistant message and updating the session status. When the frontend received `done`, it cleared the live streaming buffer and refetched the history. That refetch raced ahead of the `MessageStore::create` insert and returned the **old** history (without the new message) — so the streaming bubble vanished while the persisted message hadn't appeared yet, producing the flash.

2. **Frontend buffer-wipe on `connected`**: `useSession`'s SSE handler wiped the live buffer to `EMPTY_LIVE_BUFFER` on every `connected` event. The SSE server emits `connected` on every (re)connect — including the natural reconnect that happens when the server closes the stream at the end of a turn and the browser's `EventSource` reopens it. So a few ms after `done` arrived, `connected` fired and the buffer was wiped again, erasing the streamed text.

**Fix:**

- **Backend** (`crates/weave-server/src/service/sessions.rs`): capture the `Done` event's `stop_reason` when it arrives from the agent, do not broadcast it inline, and emit it **after** the assistant message has been persisted and the session status updated. This closes the race: by the time the client sees `done`, the new row is in the database and the history refetch will find it.
- **Frontend** (`web/src/hooks/use-session.ts`): make the `connected` event a no-op. The SSE server already handles replay via `Last-Event-ID`, so the client doesn't need to wipe state to "clear stale chunks." The buffer is cleared on `done` instead, which now happens after persistence.

**Verification:** All 3 init.sh layers pass; 327 Rust tests + 21 frontend tests; lint/clippy/fmt clean.

**Files changed:** `crates/weave-server/src/service/sessions.rs` (+22/-5), `web/src/hooks/use-session.ts` (+10/-2)

### 2026-06-01 — Bug fix: page flash on done (round 2)

**Symptom:** The earlier round-1 fix (persist-before-broadcast, no `connected` wipe) didn't fully eliminate the flash. The chat still had a brief missing-bubble moment after the agent finished.

**Root cause:** The `done` handler was clearing the live buffer (`textChunks: []`, `isStreaming: false`, etc.) **before** the history refetch had a chance to return the new assistant message. Result: the live bubble vanished, and a moment later the persisted message appeared. The brief gap was the flash.

**Fix:**

- **`useSession` `done` handler** (`web/src/hooks/use-session.ts`): instead of resetting the buffer to `EMPTY_LIVE_BUFFER`, just flip `isStreaming` to `false` and `stopReason` to the event's value. The accumulated `textChunks`, `toolCalls`, and `thinkingChunks` stay in place. The `LiveAssistantMessage` continues to render the same content until the persisted message arrives.
- **`LiveAssistantMessage` dedup** (`web/src/app/pages/session.tsx`): accept the latest persisted assistant message's `content` as a prop. If the live buffer's `textChunks.join("")` matches the persisted content exactly, return `null` — the persisted `AssistantMessage` is now rendering the same text, so showing the live bubble would duplicate it. This gives a smooth handoff: live bubble visible until persisted arrives, persisted bubble visible after, no gap and no duplicate.

**Why content equality is the right key:** the backend's `run_prompt_task` stores `accumulated` (`service/sessions.rs`) which is the concatenation of every `TextDelta` it broadcast over SSE — i.e. the same string the live buffer accumulates. The comparison is exact.

**Verification:** All 3 init.sh layers pass; 327 Rust tests + 21 frontend tests; lint/clippy/fmt clean.

**Files changed:** `web/src/hooks/use-session.ts` (+8/-7), `web/src/app/pages/session.tsx` (+12/-1)

### 2026-06-01 — Feat: session flow re-implementation (feat-036)

**Symptom:** The chat had four persistent UX problems: (a) flash on turn completion, (b) duplicate bubble on reload, (c) first-token latency with no skeleton, (d) cancel/error showing nothing. The user requested a re-implementation rather than incremental fixes.

**Approach:** New `SseWireEvent::MessagePersisted` event carries the persisted row's database id from the server *after* `MessageStore::create` and *before* the terminal `done`. The frontend's `useReducer` swaps the live bubble for the persisted one by id — never by string comparison. Partial text is now persisted on cancel and on streaming error, with `stop_reason` stored in the existing `messages.metadata` JSON column. 24 unit tests pin the reducer contract; the SSE wire event is covered by 3 new Rust tests.

**Files changed:** `crates/weave-server/src/sse/mod.rs` (+93), `crates/weave-server/src/service/sessions.rs` (+30/-2), `web/src/hooks/use-session.ts` (full rewrite around `useReducer`), `web/src/hooks/__tests__/use-session.test.ts` (new, 24 tests), `web/src/lib/types.ts` (+24), `web/src/app/pages/session.tsx` (+225/-24)

### 2026-06-01 — Bug fix: page flash on completion (feat-036 round 2)

**Symptom:** Even with the id-based handoff protocol from feat-036, the chat still flashed after the agent finished. The user saw: streamed text → blank gap → re-rendered text. The "reload" effect made it feel like the response was being cleaned out and reloaded.

**Root cause:** The `liveSuperseded` gate in `LiveAssistantMessage` was `liveBuffer.streamId !== null && liveBuffer.persistedTurnId !== null`. This hid the live bubble the moment the `message_persisted` SSE event landed — but the `AssistantMessage` doesn't render until the history refetch resolves, which is one network round-trip later. The gap between "live bubble hides" and "persisted bubble appears" was the flash. Additionally, the persisted bubble's `animate-fade-in-up` (300ms upward slide) read as a "reload" of the same content.

**Fix:**

- **`SessionPage`** (`web/src/app/pages/session.tsx`): compute `livePersistedMessage = messages.find(m => m.id === liveBuffer.persistedTurnId)` via `useMemo`. Pass it to `LiveAssistantMessage`. The new gate is `liveSuperseded = persistedMessage !== undefined` — the live bubble hides only when the persisted row is actually rendered, making the swap atomic.
- **`AssistantMessage`**: new optional `skipAnimate` prop suppresses `animate-fade-in-up` when the message is the one that just replaced a live bubble (`msg.id === liveBuffer.persistedTurnId`). Every other mount (initial page load, navigating to a session) still gets the fade-in.
- **`LiveAssistantMessage`**: takes the new `persistedMessage: Message | undefined` prop; the dedup check is now purely on whether the persisted row is in the messages array, not on the reducer's transient `persistedTurnId` state.

**Verification:** 24 reducer unit tests + 333 Rust tests pass; prettier clean; `./init.sh` all 3 layers green. Browser smoke: 3-turn session (cat story → haiku → "What's 2+2?") rendered cleanly with no flash, no "Thinking..." indicator on the persisted message, and a "Ready" status badge immediately after the agent finished.

**Files changed:** `web/src/app/pages/session.tsx` (+18/-3)

### 2026-06-01 — Bug fix: stale "Thinking..." badge after turn ends

**Symptom:** After the agent finished a turn, the chat bubble continued to display a "Thinking..." badge in its header, even though the full response text was already shown beneath it.

**Root cause:** The badge is gated on `isStreaming === true`, set by the SSE `text_delta`/`tool_use_start`/`thinking` events and reset by the `done` event. In the round-2 fix, the live buffer was kept across `done` so the bubble stayed visible during the refetch. But the bubble now shows a stale "Thinking..." badge when:

- The `done` event arrives but the live buffer still has `isStreaming: true` from a prior `text_delta` that was processed in the same event loop tick (ordering edge case in the SSE handler), OR
- The persisted text differs from the live text by trailing whitespace or a final newline (e.g., the Anthropic API appends a trailing `\n` to the response), so the dedup check `persisted === text` fails, and `isStreaming` is `true` because the `done` event reset is masked by a subsequent `text_delta` from a buffered replay.

**Fix (defense in depth in `LiveAssistantMessage`):**

1. **Trim before comparing.** `persistedAssistantContent.trim() === text.trim()` instead of strict equality. The backend stores the concatenated `text` deltas, and any trailing whitespace difference (real or perceived) no longer blocks the dedup.
2. **Gate the badge on `!persistedMatches`.** Even if `isStreaming` is `true`, the badge is hidden when the persisted content has caught up. This is a belt-and-suspenders guard: the persisted-message dedup at the top of the function still returns `null` and hides the entire bubble, but if a future code path keeps the bubble rendered with mismatched content, the badge at least won't lie.

**Verification:** All 3 init.sh layers pass; 327 Rust tests + 21 frontend tests; lint/clippy/fmt clean.

**Files changed:** `web/src/app/pages/session.tsx` (+4/-2)

## Out-of-Scope Items Noticed

- **Cancel button in session header** (`session.tsx:582-593`) is visible whenever status is `"connecting"` or `"ready"`, including between turns. Clicking it with no active stream hits the backend's `cancel_session` validation. Visible-but-no-op is a UX wart. Defer to a follow-up: hide based on `liveBuffer.isStreaming` rather than just status.
