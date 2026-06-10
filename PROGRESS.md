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
- **Latest commit:** `760b24a chore: add precommit hook enforcing just check` (this session)
- **Previous commit:** `cfb8c02 feat(phase-7): runtime x mode compatibility validator (feat-040)` (committed before this session; prior PROGRESS.md was stale at session start)
- **Active feature:** none — small chore session: added the version-controlled precommit hook + init.sh activation block. 6 verification steps ran end-to-end: hook standalone passes, init.sh sets `core.hooksPath`, deliberate test-panic gets rejected with the FAIL block, `--no-verify` escape hatch works, working tree clean, full `./init.sh` 3-layer gate stays green. See the session entry in `PROGRESS-archive.md` for full verification trace.
- **In-flight (uncommitted):** none — working tree clean
- **Build status:** green — `./init.sh` 3-layer gate passes
- **Test status:** green — 659 Rust tests + 113 frontend tests
- **Lint status:** green — clippy clean, fmt clean, prettier clean, ESLint clean
- **Precommit hook:** active on this clone (`core.hooksPath = .githooks`). Every `git commit` now runs `just check` and aborts on failure. CLAUDE.md hard constraint #9 is enforced mechanically. Bypass with `git commit --no-verify` when needed.

## Next Steps (in order)

1. **Pick the next `not_started` phase-7 feature.** Per the `feature_list.json` next-features list: `feat-041` (TurnContext extension to CodingAgent trait) has dependencies feat-005/009/038 all `passing` and is the natural next step in the multi-runtime foundation. `feat-042` (Per-Runtime-Tool model cache) depends on feat-005/007/039 and is independent of feat-041. With the precommit hook in place, the next feature implementation will be gated at commit time — green builds required.
2. **(Low priority, ask first) Clean up untracked backup files at the repo root** — `weave.db.bak.20260609-110204` and `weave.db.bak.20260609-160418` (carry-over from the 2026-06-09 data cleanup and fix-068 recovery). Confirm with the user, then `rm` them. Do not delete the `weave.db` itself.

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
- **`AppError::Validation` is flat (feat-040 decision)**: the runtime × mode mismatch payload (runtime, mode, supported modes) is encoded in the `message` string, not as a structured `details` field. `feat-053` (session-creation wizard) will need to regex the message when surfacing the error. If a future feature needs a structured payload, add a new `AppError::ValidationWithDetails` variant project-wide (the `cwd_outside_codebase` spec in feat-050 also anticipates the same shape).
- **feat-050 ordering note (feat-040 review)**: `try_automate_lane` routes through `SessionService::create_session`, so the `validate_runtime_mode_compat` call inside the chokepoint fires *before* the codebase check in feat-050. A wrapped session will pass feat-040's matrix first, then hit the codebase check second — the order is correct as-is.

## Session-Start Tips

See `CLAUDE.md` for full conventions. Highlights:

- Package manager is **Bun** (not npm)
- `./init.sh` is the one-command full verification gate (run before and after any change)
- `feature_list.json` is the single source of truth — do not track work in comments/TODOs
- `docs/user/` is the user-facing documentation set
- `ci status` first, then `ci orient`/`ci pack`/`ci find` for code exploration (5 primitives + 7 recipes; see `CLAUDE.md` § Module Index)
