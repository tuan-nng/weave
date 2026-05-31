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

## Topic Docs

- `docs/ARCHITECTURE.md` — System layout, domain model, API surface, SQLite schema
- `docs/SYSTEM_DESIGN.md` — Implementation details, service contracts, tool definitions, security model
- `docs/PLAN.md` — Implementation phases and file creation order

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
