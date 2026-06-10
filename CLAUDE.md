# CLAUDE.md

<!--
ROUTING FILE — keep this under 200 lines.
This is a map, not an encyclopedia. Detail goes in topic docs.
-->

Weave is a web-based multi-agent coordination platform — a single Rust binary (Axum) serving a React SPA, backed by SQLite. It orchestrates AI coding agents through structured sessions, streaming, and kanban-driven workflows.

## Quick Start

```bash
./init.sh           # full verification (deps + lint + tests + build)
just dev             # start backend dev server (cargo watch)
just dev-web         # start frontend dev server (vite)
just check           # fast verification (lint + tests, no build)
just test            # run all tests (Rust + frontend)
```

If `./init.sh` fails on a fresh clone, fix that before any feature work.

## Startup Workflow

Before writing code, the agent MUST:

1. Run `pwd` to confirm working directory is repo root
2. Read this file completely
3. Read `PROGRESS.md` for current state
4. Read `DECISIONS.md` for non-obvious design choices
5. Read `feature_list.json` to find the next `not_started` feature
6. Run `./init.sh` to confirm the repo is in a consistent state

If baseline verification is failing, repair that first. Do not stack new work on a red repo.

## Working Rules

- **WIP = 1.** Exactly one feature in `active` state at any time. Pick from `feature_list.json`.
- **Verification gates state.** A feature moves from `active` to `passing` only after its verification command succeeds. The agent does not edit state directly.
- **State file lifecycle.** `PROGRESS.md` is a **rolling** file, target ~80 lines. After committing a feature/fix, move its detailed journal entry to `PROGRESS-archive.md`. `PROGRESS.md` holds only: `## Current State` + `## Next Steps` + quick architectural pointer + active out-of-scope list. `PROGRESS-archive.md` holds the full session entry, the completed-features list, and the session-notes timeline (append-only — never delete historical entries). If `PROGRESS-archive.md` grows beyond ~1500 lines, split by quarter (e.g. `PROGRESS-archive-2026-Q2.md` with the latest quarter always in `PROGRESS-archive.md`).
- **No refactoring before core verification.** Functional correctness first, then performance, then style.
- **Stay in scope.** Do not modify files unrelated to the active feature. If you find an unrelated issue, log it in `PROGRESS.md` under "Out-of-Scope Items Noticed" — do not fix it inline.
- **Leave clean state.** Every session ends with the exit checklist green (see below).

## Hard Constraints

1. All database queries use `rusqlite` with parameterized queries — no string interpolation in SQL
2. All public API endpoints return JSON with consistent `{ "data": ... }` or `{ "error": ... }` shape
3. SSE is the only real-time transport — no WebSocket
4. The binary embeds the frontend at build time (`build.rs`) — no separate static file server in production
5. All state is workspace-scoped — every query must include `workspace_id`
6. No ORM — raw `rusqlite` with manual mapping
7. Async runtime is `tokio` — no blocking calls in async handlers
8. Frontend data fetching via TanStack Query — no manual `fetch` in components
9. All commits must pass `./init.sh`
10. `feature_list.json` is the single source of truth for task scope — do not track work in comments or TODOs

## Module Index

Use `ci sketch <file> -p weave` for declarations, `ci show <sym> --with-body -p weave` for implementations. Jump directly to the relevant module — don't scan.

| Directory | Contains | Key symbols |
|---|---|---|
| `crates/weave-server/src/api/` | HTTP handlers (Axum routes) | Router assembly in `mod.rs` |
| `crates/weave-server/src/store/` | SQLite data access (rusqlite) | `*Store` structs, `kanban_test_helpers.rs` |
| `crates/weave-server/src/service/` | Business logic orchestration | `SessionService`, `try_automate_lane`, `check_transition_gates` |
| `crates/weave-server/src/tools/` | Agent tool implementations | `ToolExecutor` trait, `ToolRegistry`, profiles in `mod.rs` |
| `crates/weave-server/src/sse/` | SSE infrastructure | `SseManager`, `EventBuffer`, `SseWireEvent` |
| `crates/weave-server/src/a2a/` | Google A2A protocol v1.0 | Agent Card, SendMessage, GetTask, SubscribeToTask |
| `crates/weave-server/src/trace/` | Trace collection | `TraceCollector`, `extract_file_changes` |
| `crates/weave-server/src/specialist/` | Specialist YAML loading | `SpecialistRegistry` |
| `crates/weave-server/src/` | App entry + foundations | `main.rs`, `db.rs`, `error.rs`, `config.rs` |
| `web/src/app/pages/` | React page components | Route-level pages + sub-components |
| `web/src/hooks/` | TanStack Query hooks | `useSession`, `useBoard`, `useJourney`, etc. |
| `web/src/lib/` | Frontend infrastructure | `types.ts`, `api.ts`, `query-keys.ts`, `routes.ts` |

**Per-module index files:** Each directory above contains a `_INDEX.md` with file listings, key symbols, sizes, and connection maps. Read the `_INDEX.md` before scanning source files — it replaces directory-wide exploration. Workflow: pick module from this table → read `_INDEX.md` (50-80 lines) → `ci show <sym>` for implementation.

## Topic Docs

Load only the one relevant to your task — don't read the whole set. Start with `SYSTEM_DESIGN.md` for the routing map, then drill into specifics.

### Road-map
Forward-looking plans and strategies. Read these when deciding *what to build next* or *why we're going in a direction*.

- `docs/road-map/PLAN.md` — Implementation phases and file creation order (the original v1 plan, now historical)
- `docs/road-map/multi-runtime-strategy.md` — Strategy for adding Claude Code / Codex / OpenCode as session runtimes (the active strategic direction)

### Current state
Reference material for the system *as it exists today*. Load only the one relevant to your task.

- `docs/SYSTEM_DESIGN.md` — Routing map: architecture layers, session state machine, what was dropped. **Start here.**
- `docs/ARCHITECTURE.md` — System layout, domain model overview
- `docs/data-model.md` — Load when adding/modifying DB tables or understanding schema
- `docs/api-contracts.md` — Load when adding/modifying API endpoints or SSE events
- `docs/domain-services.md` — Load for service orchestration, session lifecycle, specialist loading
- `docs/provider-abstraction.md` — Load when adding tools, providers, tool profiles, or security constraints
- `docs/sse-design.md` — Load for SSE infrastructure, reconnection, backpressure
- `docs/kanban-automation.md` — Load for lane automation flow or board templates
- `docs/error-handling.md` — Load for error types, HTTP mapping, retry strategy
- `docs/concurrency-model.md` — Load for async task spawning, shared state, shutdown
- `docs/frontend-architecture.md` — Load for component trees, hooks, SSE→cache sync
- `docs/operations.md` — Load for dependencies, security, logging, backup, performance

## Definition of Done

A feature is done only when ALL of these hold (the three-layer gate):

1. **Static (Layer 1):** `cargo clippy` clean, `cargo fmt --check` passes, frontend type-checks
2. **Behavior (Layer 2):** Unit and integration tests pass; the binary starts and reaches ready state
3. **System (Layer 3):** End-to-end verification command from `feature_list.json` succeeds

Skipping a layer = not done. Layer 2 doesn't begin until Layer 1 passes. Layer 3 doesn't begin until Layer 2 passes.

Evidence (commit hash, test output excerpt) must be recorded in `feature_list.json`.

## Session Exit Checklist

Before ending a session, the agent MUST confirm all five dimensions:

- [ ] **Build** — `./init.sh` passes
- [ ] **Tests** — all green, including pre-existing tests
- [ ] **Progress** — `PROGRESS.md` and `feature_list.json` reflect actual state
- [ ] **Artifacts** — no debug logs (`dbg!`, `println!`, `console.log`), commented-out code, stale temp files, or unmoored TODOs
- [ ] **Startup** — `./init.sh` works from a clean clone of the current commit

If any check fails, finish or revert before exiting. "Clean up next session" means never.

**Enforcement:** A `Stop` hook (`.claude/hooks/session-exit-check.sh`) blocks the session from ending when work was done but `PROGRESS.md` wasn't updated or `feature_list.json` has stuck in-progress entries.

## Escalation

- **Architecture decisions** → consult `docs/ARCHITECTURE.md` or ask the user
- **Unclear requirement** → check `feature_list.json` behavior description; if still unclear, ask
- **Repeated test failure (same root cause, 3+ retries)** → stop, log in `PROGRESS.md`, ask
- **Verification command itself broken** → fix `init.sh` or the test, log the fix in `DECISIONS.md`

## Error Message Style

When you write error messages, lint rules, or test failures that the next agent session will read, include three elements:

1. **What** went wrong (specific, with file/line)
2. **Why** the rule exists
3. **How** to fix it (concrete steps)

Bad: `Test failed`
Good: `Test failed: GET /api/workspaces returned 500. SQLite WAL mode not initialized — check db.rs:init_db() runs before routes are mounted.`
