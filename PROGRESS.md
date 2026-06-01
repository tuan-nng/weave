# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-06-01
- **Latest commit:** e884448 (feat-016)
- **Active feature:** none
- **Build status:** green — `cargo build -p weave-server` succeeds
- **Test status:** green — 297 tests pass (38 new for feat-016 + 259 existing)
- **Lint status:** green — clippy clean, fmt clean

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

## In Progress

(none)

## Blocked

(none)

## Known Issues

- `web/` directory does not exist yet (expected — Phase 3)

## Next Steps

1. Start feat-016: Task context tools (depends on feat-012 — passing)
2. Continue Phase 2 (Agent Tools & Observability)

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

## Notes for Next Session

- feat-016 created: `src/tools/task/` directory (5 files), `src/store/tasks.rs`
- `ToolContext` now has `workspace_id` field — all future tools should use it for workspace scoping
- Task tools are the first to hold `Arc<Db>` — pattern for future DB-accessing tools
- Tool registration in `main.rs`: 14 tools now (5 fs + 1 shell + 4 git + 4 task)
- `implementation` profile has 15 tools listed, 14 now registered (artifacts still pending)
- `review` profile has 10 tools listed (3 git tools, no git_commit, 3 task tools)
- Next feature: feat-017 (TraceCollector) — depends on feat-009, feat-012

## Out-of-Scope Items Noticed

(none yet)
