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
- **Latest commit:** `15dc466 chore: trim PROGRESS.md, add PROGRESS-archive.md, document state-file lifecycle` (no new commit yet — feat-040 implementation complete, about to be committed in this session)
- **Active feature:** none — `feat-040` moved to `passing` this session. 7-phase `feature-dev` workflow ran end-to-end: Phase 1 (Discovery, from prior session), Phase 2 (Codebase exploration, 3 code-explorer reports preserved in PROGRESS-archive.md from prior session), Phase 3 (Clarifying questions, user confirmed via "your call" → 3 decisions: hybrid A2A extend + chokepoint test, message-string payload, terse attended message; spec also fixed to match shipping enum names), Phase 4 (Architecture, 3 code-architect agents, user selected Pragmatic), Phase 5 (Implementation, 5 files), Phase 6 (Quality review, 3 reviewers — Correctness + Conventions passed; Simplicity flagged 3 issues, 2 applied: redundant A2A test assertion dropped, 2 supported_modes tests consolidated to 1), Phase 7 (Summary, this entry).
- **In-flight (uncommitted):** `feature_list.json` (feat-040 → `passing` + evidence), `crates/weave-server/src/agent/mod.rs` (validator + 6 tests), `crates/weave-server/src/a2a/types.rs` (2 optional fields), `crates/weave-server/src/a2a/messages.rs` (call-site + 2 tests), `crates/weave-server/src/service/sessions.rs` (chokepoint call + 1 test), `PROGRESS.md` (this file, current state + OOS updates)
- **Build status:** green — `./init.sh` 3-layer gate passes
- **Test status:** green — 659 Rust tests + 113 frontend tests, 9 new tests in feat-040
- **Lint status:** green — clippy clean, fmt clean (auto-fixed), prettier clean, ESLint clean

## Next Steps (in order)

1. **Commit feat-040.** All 5 implementation files are modified, `feature_list.json` is flipped to `passing` with evidence, `./init.sh` is green. Stage and commit with a feat message. Then `git push` and the user reviews the diff.
2. **Pick the next `not_started` phase-7 feature.** Per the `feature_list.json` next-features list: `feat-041` (TurnContext extension to CodingAgent trait) has dependencies feat-005/009/038 all `passing` and is the natural next step in the multi-runtime foundation. `feat-042` (Per-Runtime-Tool model cache) depends on feat-005/007/039 and is independent of feat-041.
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
- **`AppError::Validation` is flat (feat-040 decision)**: the runtime × mode mismatch payload (runtime, mode, supported modes) is encoded in the `message` string, not as a structured `details` field. `feat-053` (session-creation wizard) will need to regex the message when surfacing the error. If a future feature needs a structured payload, add a new `AppError::ValidationWithDetails` variant project-wide (the `cwd_outside_codebase` spec in feat-050 also anticipates the same shape).
- **feat-050 ordering note (feat-040 review)**: `try_automate_lane` routes through `SessionService::create_session`, so the `validate_runtime_mode_compat` call inside the chokepoint fires *before* the codebase check in feat-050. A wrapped session will pass feat-040's matrix first, then hit the codebase check second — the order is correct as-is.

## Session-Start Tips

See `CLAUDE.md` for full conventions. Highlights:

- Package manager is **Bun** (not npm)
- `./init.sh` is the one-command full verification gate (run before and after any change)
- `feature_list.json` is the single source of truth — do not track work in comments/TODOs
- `docs/user/` is the user-facing documentation set
- `ci status` first, then `ci orient`/`ci pack`/`ci find` for code exploration (5 primitives + 7 recipes; see `CLAUDE.md` § Module Index)
