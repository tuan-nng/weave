# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-06-01
- **Latest commit:** 8cb76ab (UI redesign)
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

## In Progress

(none — ready for next feature)

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

## Out-of-Scope Items Noticed

(none yet)
