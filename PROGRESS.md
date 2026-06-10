# PROGRESS.md

<!--
The amnesiac craftsman's journal. **Rolling file, target ~80 lines.**

Updated at session start (read it) and session end (rewrite it). A fresh
session should reach executable state in <3 minutes by reading this file.

What goes here:
  - ## Current State — canonical state of the project
  - ## Next Steps — what the next session should do (1-3 items)
  - ## Key Architectural Decisions (quick ref) — pointer to DECISIONS.md
  - ## Out-of-Scope Items Noticed (active) — small list of open items

What does NOT go here:
  - Detailed journal entries for committed features/fixes → PROGRESS-archive.md
  - The "Completed Since Project Start" list → PROGRESS-archive.md
  - Historical session notes → PROGRESS-archive.md
  - Full "Remaining Features" table → feature_list.json

For session entries, completed-features list, session-notes timeline, and
the full out-of-scope list, see PROGRESS-archive.md.
For non-obvious architectural decisions with rationale, see DECISIONS.md.
For the full feature list and verification gates, see feature_list.json.
For session-start tips and conventions, see CLAUDE.md.
-->

## Current State

- **Last updated:** 2026-06-10
- **Latest commit:** `15dc466 chore: trim PROGRESS.md, add PROGRESS-archive.md, document state-file lifecycle`
- **Active feature:** none — phase-7 multi-runtime strategy: feat-038 (committed `1dfabeb`) and feat-039 (committed `075e721`) are both `passing`; fix-069 is committed (`40b5032`); harness improvement (PROGRESS.md trim + archive + lifecycle rule) committed in `15dc466`; ready to pick the next `not_started` phase-7 feature.
- **In-flight (uncommitted):** none
- **Build status:** green — `./init.sh` all 3 layers pass
- **Test status:** green — 650 Rust tests (642 pre-feat-039 + 8 for the kind-discriminated union) + 113 frontend tests pass
- **Lint status:** green — clippy clean, fmt clean, prettier clean, ESLint clean
- **Uncommitted:** none

## Next Steps (in order)

1. **Pick the next `not_started` phase-7 feature** from `feature_list.json`. Likely candidates in dependency order: `feat-040` (runtime×mode validation matrix), `feat-041` (per-turn `TurnContext`), or `feat-042` (per-adapter model cache — the 501 branch on `list_provider_models` for `kind=cli` rows added in feat-039 lands its first user here).
2. **Set the chosen feature to `active`** in `feature_list.json` and proceed with the standard 7-phase feature-dev workflow (`/feature-dev:feature-dev start feat-NNN`).
3. **(Low priority, ask first) Clean up untracked backup files at the repo root** — `weave.db.bak.20260609-110204` and `weave.db.bak.20260609-160418` (carry-over from the 2026-06-09 data cleanup and fix-068 recovery). Confirm with the user, then `rm` them. Do not delete the `weave.db` itself.

## Key Architectural Decisions (quick reference)

See `DECISIONS.md` for full rationale. Quick reference:

- Single Rust binary with embedded frontend (build.rs)
- SQLite with WAL mode, no ORM (raw rusqlite)
- SSE for all real-time (no WebSocket)
- Workspace-scoped resources (every query includes `workspace_id`)
- `feature_list.json` is the single source of truth for task scope

## Out-of-Scope Items Noticed (active)

Items deferred from past sessions. Address when a feature touches the relevant area. Full historical list: see `PROGRESS-archive.md` § Out-of-Scope Items Noticed.

- `verify_task_in_workspace` duplicated across `store/artifacts.rs`, `service/kanban.rs`, `api/kanban.rs` — fix: add `TaskStore::workspace_id_for_task`
- Unmatched `/api/*` paths return `index.html` instead of 404 JSON — fix: nest API router under `/api` with JSON 404 handler
- `SseManager` channel GC: no cleanup for stale board/session channels on long-running servers
- `MAX_TASK_TITLE_LEN` defined in two places: `tools/fs/mod.rs` and `api/kanban.rs` — fix: hoist to `store::tasks`
- `type_complexity` clippy warning in `service/sessions.rs:1436` (test helper) — doesn't fail `just lint` because lint runs without `--all-targets`
- **Tool-containment partial gap (security audit, feat-037 review)**: shell-body jail is by-design NOT enforced even for bound sessions — the `cwd`-arg / `fs_*` / explicit-`cwd` form of shell+git containment is enforced (feat-062), but a shell command body is not. The `docs/user/sessions.md` "How sessions use a codebase" section documents this trade-off explicitly.

## Session-Start Tips

See `CLAUDE.md` for full conventions. Highlights:

- Package manager is **Bun** (not npm)
- `./init.sh` is the one-command full verification gate (run before and after any change)
- `feature_list.json` is the single source of truth — do not track work in comments/TODOs
- `docs/user/` is the user-facing documentation set
- `ci status` first, then `ci orient`/`ci pack`/`ci find` for code exploration (5 primitives + 7 recipes; see `CLAUDE.md` § Module Index)
