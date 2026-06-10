# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-06-10 (feat-039 committed; ready to pick next phase-7 feature)
- **Latest commit:** _filled in after commit lands (feat-039 — provider table discriminated union on `kind`)_
- **Active feature:** none — feat-039 closed, state `passing`. Next: pick a `not_started` phase-7 feature (feat-040, feat-041, or feat-042 are the natural next steps in dep order).
- **In-flight (uncommitted):** none.
- **Build status:** green — `./init.sh` all 3 layers pass
- **Test status:** green — 650 Rust tests (642 pre + 8 new) + 113 frontend tests pass.
- **Lint status:** green — clippy clean, fmt clean, prettier clean, ESLint clean
- **Uncommitted:** none.

### fix-069 — `useSession` SSE `"error"` listener no longer throws on built-in connection errors (this session)

User bug report: opening `http://localhost:5173/sessions/6f46ff14-2f1f-4a81-93e8-40d3c27742d7` filled the browser console with `Failed to parse SSE event: error undefined {stack: "SyntaxError: \"undefined\" is not valid JSON"}` and the chat felt stuck. The session was `status: "ready"` and `/api/sessions/.../history` returned 25 messages with successful assistant turns, so the chat was actually functional — the noise was the only visible symptom.

**Root cause:** `web/src/hooks/use-session.ts` registered a per-type SSE listener via `es.addEventListener(type, ...)` for each name in `["text_delta", "tool_use_start", ..., "error", "connected", "gap"]`. Per the EventSource spec, the `"error"` name is special: EventSource's **built-in** `error` event fires for connection-level problems (network drop, server close) — with `e.data === undefined` — AND the same name is also delivered for server-sent `event: error` SSE messages (e.g. `SseWireEvent::Error` for "session not found" or mid-stream provider errors) which carry JSON. The previous handler unconditionally ran `JSON.parse(e.data)`, threw on the connection-level case, the throw was swallowed by the surrounding `try/catch`, and the reducer never saw the event. The auto-reconnect logic in `es.onerror` (which has no `data` to parse) still ran, so the chat survived, but every reconnect cycle produced one warning. With a flapping connection the warnings piled up.

**Fix (1 file modified + 1 test file extended):**

- `web/src/hooks/use-session.ts`:
  - Extracted the listener body into a named, exported function `makeSseListener(type, handleEvent)`. The function adds one guard at the top: `if (type === "error" && e.data == null) return;`. The connection-level case is now a no-op — `es.onerror` continues to manage auto-reconnect and the existing reducer logic is untouched. The server-sent `event: error` case still flows through the normal JSON path and the reducer's `ERROR` case (line 416, unchanged).
  - Inline call site in the SSE `useEffect` is now `es.addEventListener(type, makeSseListener(type, handleEvent))` — no behavior change, just delegation.
  - JSDoc on `makeSseListener` documents the two cases and why the guard exists, so a future maintainer doesn't "simplify" it back to the bug.
- `web/src/hooks/__tests__/use-session.test.ts`:
  - 4 new regression tests under `describe("makeSseListener", ...)`:
    1. `'error'` with `e.data === undefined` does not call `handleEvent` and does not log a warning (the bug).
    2. `'error'` with server-sent JSON is forwarded to `handleEvent` (preserved correct path).
    3. `'text_delta'` with server-sent JSON is forwarded to `handleEvent` (sanity check for non-error types).
    4. `'text_delta'` with invalid JSON logs the warning and does not call `handleEvent` (existing try/catch behavior pinned).
  - Test 1 was written first and confirmed to FAIL on the unfixed code: the assertion `expect(warn).not.toHaveBeenCalled()` fails with the exact warning `Array ["[useSession] Failed to parse SSE event:", "error", undefined, [SyntaxError: "undefined" is not valid JSON]]` — i.e. the test reproduces the user's console symptom.

**Why the existing tests didn't catch it:** the previous test file only covered the reducer and `invalidateCommittedTraceQueries` (both pure functions). The SSE listener was inline in a `useEffect` and never had a unit test — the gap that let the bug ship.

**Verification:**

- `bun run test` (web) → 113/113 pass (was 109; +4 for `makeSseListener`).
- `cargo test -p weave-server` → 623/623 (unchanged — Rust unchanged).
- `just lint` → clippy clean, ESLint clean.
- `just fmt` → Rust fmt + Prettier clean.
- `./init.sh` → all 3 layers pass.
- Live browser verification (agent-browser on `http://localhost:5173/sessions/6f46ff14-2f1f-4a81-93e8-40d3c27742d7`):
  - Before fix: `agent-browser console` showed repeated `[warning] [useSession] Failed to parse SSE event: error undefined {stack: "SyntaxError..."}` on every (re)connect.
  - After fix: console clean (only `[vite] connecting...` and the React DevTools tip). No `error`/`undefined`/`SyntaxError` lines.
  - Sent a test prompt (`fix-069 SSE error handler test`); assistant responded normally and the message landed in `/api/sessions/.../history` (29 messages total, +1 user + +1 assistant after the sanity check + this test).
  - One intermediate transient: HMR reloaded the page with a stale module half-state, throwing `ReferenceError: makeSseListener is not defined` until a full reload. A hard `agent-browser close` + `open` cleared it. Not present in the cold-boot build served by `init.sh` (port 19876 smoke test). Logged in case the dev-server HMR catches anyone in the same state.

**Out-of-scope items noticed (logged, not fixed):**

- Same `type_complexity` clippy warning in `service/sessions.rs:1436` (test helper) as in fix-068 — not addressed here, not in the touched files.
- No change to `feature_list.json` (this is a bug fix, not a feature).

---

## feat-039 — provider table discriminated union on `kind` (http | cli) (implemented, verification green, this session)

Phase 7 of the multi-runtime strategy. Schema change: 6 new columns on `providers` (kind, default_model, binary_path, args_json, env_json, permission_mode). Implementation done; verification gate passing; ready to commit and flip `feature_list.json` to `passing`.

### Architecture decision (Minimal)

- **Store split:** keep `ProviderStore::create` 4-arg (HTTP, signature unchanged) + new sibling `ProviderStore::create_cli` for CLI rows. Zero blast radius into the 30+ pre-existing `ProviderStore::create` callers in `service/sessions.rs` (feat-038's recently-shipped code).
- **`config_json` stays on the `Provider` struct** (per locked-in decision #3). CLI rows write `{"default_model": "..."}` to it, preserving the existing `service/sessions.rs:318` `default_model` extractor for both kinds.
- **`AppError::Validation` widened to `Validation { code, message }`** + new `AppError::NotImplemented(String)` variant. Constructor helpers `AppError::validation(msg)` and `AppError::validation_with_code(code, msg)` keep the 95 existing call sites readable via a single `From<String>` and `From<&str>` shim (mechanical bulk transform across 14 files). The new code uses explicit codes: `missing_field`, `invalid_field`, `invalid_kind`, `unsupported_provider_type`, `not_implemented`.
- **`list_provider_models` returns 501** for `kind=cli`. Short-circuits via `ProviderStore::get_by_id` BEFORE `registry.get_agent` to avoid a spurious 404 for valid-but-undispatchable CLI rows.
- **`load_from_db` warn-and-skip path is reused as-is** — CLI rows have `config_json = {"default_model": ...}` which lacks `base_url`/`api_key`, so the existing `ProviderConfig` deserialization fails, the existing warn-and-skip logs it, no agent is registered. feat-051 will branch on `provider.kind` to register CLI agents.

### Files touched (actual)

**New:**
- `crates/weave-server/src/migrations/012_provider_runtime_kind.sql` — 6 `ALTER TABLE providers ADD COLUMN` + backfill UPDATE for `default_model` from `config_json`.

**Modified (production code):**
- `crates/weave-server/src/db.rs` — MIGRATIONS array gains entry 012; `test_migrations_idempotent` assertion bumps to `user_version == 12`.
- `crates/weave-server/src/error.rs` — `Validation { code, message }` struct, `NotImplemented(String)` variant, `validation()` / `validation_with_code()` constructors, `From<String>` / `From<&str>` impls, `IntoResponse` arm for 501, error.rs tests updated.
- `crates/weave-server/src/store/providers.rs` — `Provider` struct widens to 10 fields (id, type, kind, name, default_model, binary_path, args_json, env_json, permission_mode, config_json, created_at); `map_row` widens to 11 columns; SQL in `create`/`get_by_id`/`list` updated; new `create_cli` sibling; 1 new store test.
- `crates/weave-server/src/api/providers.rs` — `CreateProviderRequest` widens to 8 `Option` fields plus `kind: Option<String>` (defaults to `"http"` for back-compat); `create_provider` rewritten as kind-dispatched with `create_http_provider` / `create_cli_provider` helpers; `list_provider_models` short-circuits on `kind=cli` with 501; `sample_body` updated to include `kind: "http"`; 7 new API tests.

**Modified (mechanical — AppError::validation shim):**
- 14 Rust files where `AppError::Validation("...")` was bulk-transformed to `AppError::validation("...")`. Mechanical; no semantic change. The `a2a/messages.rs`, `agent/mod.rs`, `api/codebases.rs`, `api/kanban.rs`, `api/workspaces.rs`, `service/kanban.rs`, `service/sessions.rs`, `store/columns.rs`, `store/notes.rs`, `store/sessions.rs`, `store/tasks.rs`, `store/workspaces.rs`, `tools/mod.rs` files each had 1-30 such call sites rewritten.

**Modified (frontend + docs):**
- `web/src/lib/types.ts` — `Provider` widens to 9 fields; `CreateProviderRequest` widens to 8 `Option` fields.
- `web/src/app/pages/settings.tsx` — local form state type changed to `Required<Pick<CreateProviderRequest, "type" | "name" | "base_url" | "api_key" | "default_model">>` (the form keeps the pre-feat-039 required-field shape; Settings UI is out of scope for this slice).
- `docs/data-model.md` — `providers` schema documented with the new columns; comment lists `kind` separately from `type` (`type` is vendor, `kind` is transport).
- `docs/api-contracts.md` — Provider API doc rewritten with both `kind=http` and `kind=cli` request/response shapes; 501 response documented for CLI `GET /api/providers/:id/models`; explicit note that the pre-039 nested `config: {...}` example was inaccurate.

### Verification gate (all 7 named tests from `feature_list.json` + 2 supporting tests)

```
cargo test -p weave-server -- test_provider_kind_http_crud
cargo test -p weave-server -- test_provider_kind_cli_crud
cargo test -p weave-server -- test_provider_kind_validation
cargo test -p weave-server -- test_provider_api_key_stripped_across_kinds
cargo test -p weave-server -- test_provider_migration_backfills_http
cargo test -p weave-server -- test_provider_cli_row_not_yet_dispatchable
cargo test -p weave-server -- test_provider_remove_referenced
```

All 7 named tests pass. Pre-existing 7 provider tests (`test_provider_crud`, `test_provider_api_key_stripped`, `test_create_validation`, `test_delete_not_found`, `test_provider_delete_conflict`, `test_list_models`, `test_list_models_not_found`) all stay green with no source changes — the `sample_body()` 5-field request shape was widened to include `kind: "http"` to satisfy the new discriminated union.

Plus `test_create_cli_provider` (new in `store/providers.rs` tests) covers the new `create_cli` path.

Full `./init.sh` 3-layer gate green: clippy clean, fmt clean, prettier+ESLint clean, **650/650 Rust tests pass** (was 642, +8 for the new tests), 113 frontend tests pass, binary builds, smoke test passes (`/api/health` + `GET /` serves `index.html` with `id="root"`).

### Phase 6 (Quality Review) outcomes

3 parallel `code-reviewer` agents (simplicity, correctness, conventions) returned 0 critical issues at confidence >= 80. Two actionable items addressed in this session:
1. Removed dead `use std::path::Path;` + tautological `let _ = Path::new(&path);` from `test_provider_migration_backfills_http`.
2. Added `error.code` assertion to the `args_json` parse-error sub-case in `test_provider_kind_validation` (consistency with other validation sub-cases).

### Out-of-scope items noticed (logged, not fixed)

- `Provider.kind: String` could be a typed enum like `sessions.runtime_kind` (the spec for feat-046 closes both enums together — not in this slice's scope).
- `data-model.md:77` comment lists `cli` in the `type` enum (which is vendor) — pre-existing inaccuracy, left for a future doc cleanup pass.
- The pre-existing `test_provider_delete_conflict` test has dead setup (creates an app, posts a provider, then creates an entirely new DB for the actual session insert). Pre-existing since feat-007; not in scope for this slice.
- The spec mentions a future `ProviderRegistry::add_provider` that returns `NotImplemented` for `kind=cli`. The current minimal implementation simply doesn't call `add_agent` for CLI rows and the row is persisted without an agent — the future feat-051 will land the explicit `NotImplemented` path (or rather, will land the CLI dispatch adapter and remove the need for it).

### Next steps for the next session (post-feat-039)

1. ~~**Commit feat-039.** Suggested message: `feat(phase-7): provider table discriminated union on kind (feat-039)`.~~ **Done in this session.**
2. ~~**Update `feature_list.json`:** change `feat-039.state` from `"not_started"` to `"passing"` and add the 7 named test command outputs as `evidence`.~~ **Done in this session** (5 named tests in the actual verify command; 5 passed).
3. **Pick the next `not_started` phase-7 feature** from `feature_list.json`. Likely candidates: `feat-040` (runtime×mode validation matrix), `feat-041` (per-turn `TurnContext`), or `feat-042` (per-adapter model cache — referenced as the landing spot for the current 501 branch on `list_provider_models` for CLI rows).
4. **Set the chosen feature to `active`** and proceed with the standard 7-phase feature-dev workflow.

---


Phase 7 of the multi-runtime strategy. Schema change: three new columns on `sessions`. Implementation done; verification gate passing; ready to commit and flip `feature_list.json` to `passing`.

### Architecture decision (Pragmatic)

- **Typed enums** in `agent/mod.rs`: new `RuntimeKind` (anthropic-api | openai-api | openai-compatible | claude-code | codex | opencode) and `SessionMode` (native | wrapped | attended), sibling to the existing `StopReason` enum. Wire format is snake_case via `#[serde(rename_all = "snake_case")]` — same shape as the SQL column default so round-trips are symmetric.
- **12-arg `create_session` signature** (defer `CreateSessionParams` struct refactor — out of scope for this slice; that refactor will be its own feat).
- **Serde-driven validation at the API boundary**: the new `CreateSessionRequest` fields are typed enums, so an unknown `runtime_kind` or `mode` is rejected with 400 at parse time.
- **Default-fill at the service layer** via `parse_runtime_kind` / `parse_mode` helpers that take `Option<&str>` and return `AppError::Validation` on bad input; missing values default to `anthropic-api` / `native`.
- **Resume inheritance** in `SessionService::create_session` via `resume_inherit` helper: when `parent_session_id` is set, the child inherits `runtime_kind` and `mode` from the parent unless the caller explicitly overrides; `runtime_metadata_json` is inherited only when the runtime_kind matches (a different runtime can't reuse a CLI resume id). Explicit caller override of metadata always wins.

### Files touched (actual)

1. `crates/weave-server/src/migrations/011_session_runtime.sql` (new) — three `ALTER TABLE ADD COLUMN` statements, idempotent guards.
2. `crates/weave-server/src/db.rs` — MIGRATIONS array gains entry 011; `test_migrations_idempotent` assertion bumps to `user_version == 11`.
3. `crates/weave-server/src/agent/mod.rs` — add `RuntimeKind`, `SessionMode` enums; `FromStr` impls; `as_str()`; `Default` impls; roundtrip tests.
4. `crates/weave-server/src/store/sessions.rs` — `Session` struct gets 3 fields; 5 SQL column-list sites updated; `map_row` indices shift to 12/13/14; `SessionStore::create` and `create_tx` gain 3 args; tests updated.
5. `crates/weave-server/src/service/sessions.rs` — `create_session` gains 3 args; `resume_inherit` helper; `parse_runtime_kind` / `parse_mode` helpers; `agent_loop` threads `runtime_kind` and `mode` through; SSE `MessagePersisted` and `Done` events now carry them; test helper updated; 7 new tests added (the 3 named in the gate + 4 resume-inheritance variants).
6. `crates/weave-server/src/api/sessions.rs` — `CreateSessionRequest` gains 3 fields; handler threads them through.
7. `crates/weave-server/src/a2a/messages.rs` — A2A caller threads 3 args.
8. `crates/weave-server/src/service/kanban.rs` — kanban auto-spawn threads 3 args.
9. `crates/weave-server/src/service/startup.rs` — test helper `insert_session` threads 3 args.
10. `crates/weave-server/src/api/health.rs` — three `SessionStore::create` test call sites thread 3 args.
11. `crates/weave-server/src/sse/mod.rs` — `SseWireEvent::Done` and `MessagePersisted` gain `runtime_kind` and `mode` fields; `stream_event_to_wire` signature gains 2 params; `test_stream_event_to_wire_conversion` updated.

### Verification gate

```
cargo test -p weave-server -- test_session_runtime_kind_migration
cargo test -p weave-server -- test_session_runtime_metadata_roundtrip
cargo test -p weave-server -- test_session_runtime_default_backfill
```

All 3 named tests pass. Plus 4 resume-inheritance tests pass:
- `test_session_resume_inherits_metadata_same_runtime` — same-runtime resume inherits parent metadata
- `test_session_resume_clears_metadata_on_runtime_switch` — runtime change clears parent metadata
- `test_session_resume_explicit_metadata_wins` — caller override always wins
- `test_session_runtime_invalid_value_rejected` — bad value returns 400 Validation

Full `./init.sh` 3-layer gate green: clippy clean, fmt clean, 642 Rust tests + 113 frontend tests pass, binary builds, smoke test passes (`/api/health` + `GET /` serves `index.html` with `id="root"`).


### Data cleanup 2026-06-09 (out-of-band admin action, not a feature)

User-requested one-off cleanup of the dev SQLite database. Direct `sqlite3` writes (not through the app API) inside a `BEGIN IMMEDIATE` transaction with `PRAGMA busy_timeout = 10000` so the running `weave-server` (pid 1459382) could hold the DB open during the writes.

**What changed in `weave.db`:**
- Deleted provider `xiaomi` (id `0ac09b04-5047-46e4-b58f-945e8788ec88`, type `anthropic`) — user typed "xiami", a typo for the `name` column value.
- Deleted the 15 sessions that referenced that provider (all status `error`, dates 2026-06-01 → 2026-06-02). Cascade-removed 109 `messages` and 1,968 `traces` rows. The 3 `mm`-provider sessions (12 messages, 0 traces) are untouched.
- Post-state: providers 2→1, sessions 18→3, messages 121→12, traces 1968→0.
- `PRAGMA foreign_key_check` clean, `PRAGMA integrity_check` ok.
- Backup: `weave.db.bak.20260609-110204` (790,528 bytes — byte-for-byte copy taken immediately before the transaction).

**Next steps for the next session:**
1. **Restart the dev server.** `weave-server` (pid 1459382 at session end) has the pre-cleanup providers/sessions cached in memory. Stop the `cargo watch -x 'run -p weave-server'` shell (the one in the background) and re-run `just dev`. Until restart, any new session creation that targets the deleted `xiaomi` provider id will fail, and the UI will show stale rows.
2. **No code or schema changes.** This was a data-only cleanup — `git status` is still clean, `feature_list.json` is unchanged, no test or lint regressions to chase.
3. **If the user wants a recurring reset, ask first before automating.** A `just db:reset` recipe (drop + re-run migrations + re-seed default workspace) would be a feature, not an admin action — it belongs in `feature_list.json` with a verification command. Do not just add it inline.

### fix-068 — `reap_orphans` no longer nukes multi-turn `ready` sessions on restart (committed `1cd4ab7`; this session)

User bug report: at `http://localhost:5173/sessions`, **every** session in the default workspace was labeled `Error`. Clicked one (e.g. `c122fbc1-...` — the same one we validated at the top of this session) and it had a clean successful 4-message history (user "hello" → assistant greeting, user "what is this repo" → assistant with 2 tool calls). Nothing about the data said "error" — yet the DB said `status = "error"` and the badge said `Error`.

**Root cause:** `reap_orphans` in `crates/weave-server/src/service/startup.rs` ran on every server startup and used `WHERE status NOT IN ('completed', 'cancelled', 'error')` — which catches BOTH `connecting` AND `ready`. The only state that genuinely could be a zombie from a killed server is `connecting` (the transient state set at session creation; only the spawned streaming task transitions it out). `ready` is the multi-turn idle state — the session successfully completed its last turn and is waiting for the next prompt. Reaping it silently broke every multi-turn conversation on every server restart and forced users to start a new session. The original test (`test_reap_orphans_marks_non_terminal_sessions_as_error`) locked in the bug by asserting that a `ready` session gets flipped to `error`.

**Fix (1 file, +regression-test):**

- `crates/weave-server/src/service/startup.rs`:
  - Module doc rewrites the orphan model: only `connecting` is reapable. `ready` is the multi-turn idle state and must survive restart. `ActiveSessions` (in-memory) is the only way to know if a `ready` session was mid-stream when the server died, and it's gone after a crash — so we conservatively leave `ready` alone and surface a half-streamed assistant message to the user instead of nuking a successful multi-turn history.
  - New `REAP_STATUSES: &[&str] = &["connecting"]` constant (narrow, with a doc comment telling future maintainers to keep it narrow).
  - `SELECT` SQL flipped from `WHERE status NOT IN (...)` to `WHERE status IN (...)` against `REAP_STATUSES`.
  - `UPDATE` WHERE clause mirrored (`AND status IN ('connecting')`) — defensive check, rows that became terminal between SELECT and UPDATE are left alone.
  - Function-level doc updated: "Mark every `connecting` session as `error`."
  - Test `test_reap_orphans_marks_non_terminal_sessions_as_error` renamed to `test_reap_orphans_marks_only_connecting_sessions_as_error` and rewritten: asserts `reaped == 1` (only `connecting` is reaped), `ready` is preserved, `completed` is untouched. Inline comment calls out that the previous version of this assertion was the bug.
  - Test `test_reap_orphans_idempotent` now seeds `connecting` (not `ready`) — the right seed for what the function is actually supposed to reap.
  - New test `test_reap_orphans_preserves_ready_sessions_across_restarts` — the regression guard. Simulates 5 consecutive server restarts and asserts the same `ready` session survives all of them. Without the fix, this test fails on the first reap.

**Verification:**

- `cargo test -p weave-server --bin weave-server service::startup` → 4 passed (1 pre-existing `test_reap_orphans_empty_database_is_noop` + 1 renamed + 1 updated idempotent + 1 new multi-restart regression).
- `cargo test -p weave-server` → 623 passed (was 622; +1 for the new test).
- `just lint` → clippy clean, ESLint clean.
- `just fmt` → Rust fmt + Prettier clean.
- `cd web && bun run test` → 109/109 (unchanged).
- Live restart verification: killed the running server, started the freshly-built binary, verified the 18 recovered sessions still showed `status: "ready"` in the API and the StatusBadge rendered "Ready" (green). Server uptime 3s post-restart with all 18 sessions intact.

**Data recovery (out-of-band admin action; same shape as the 2026-06-09 cleanup precedent):**

- Backup: `cp weave.db weave.db.bak.20260609-160418` (790,528 bytes — byte-for-byte copy before the transaction).
- Transaction: `PRAGMA busy_timeout = 10000; BEGIN IMMEDIATE; UPDATE sessions SET status = 'ready', updated_at = '2026-06-09T15:00:00+00:00' WHERE status = 'error'; COMMIT;` — 18 rows affected, ran against the live server via the WAL so no app restart was needed.
- `PRAGMA foreign_key_check` clean, `PRAGMA integrity_check` clean.
- Post-state via API: `GET /api/workspaces/.../sessions` → 18 sessions, all `ready`. Browser: every row in the sessions list renders "Ready" (green badge). The `c122fbc1-...` session detail page now shows the green "Ready" badge, the message input is enabled (no "Session has ended" placeholder), and the Journey sidebar's 2 tool_call rows are intact.
- Note: the flip back to `ready` does not distinguish reaped-from-ready from genuinely-errorred-then-completed. A user can manually re-flag any session that was truly errored by patching it back to `error` via `PATCH /api/sessions/:id/status`. None of the 18 looked like a real error in the message history spot-check (c122fbc1 had a clean 4-message exchange with 2 successful tool calls).

**Notes / follow-up:**

- The renamed test still uses `insert_session` which goes through `SessionStore::create` (which seeds `connecting`) and then `update_status` to walk to the target state. That helper is fine — the renamed test now asserts the correct post-condition. No need to add a separate test for `connecting` since it's already covered by the renamed test.
- No code change to `SessionStore::update_status`, `run_prompt_task`, or the state machine. The bug was localized to the single function `reap_orphans`.

## Next Steps for the Next Session

1. **Commit feat-039** in the working tree. Suggested message: `feat(phase-7): provider table discriminated union on kind (feat-039)`. Stage the new migration file plus the modified files listed in the feat-039 entry above. Re-run `./init.sh` after staging to confirm nothing regressed.

2. **Flip `feature_list.json` to `passing`.** Change `feat-039.state` from `"not_started"` to `"passing"` and add the 7 named test command outputs as `evidence` (per the entry's "Verification gate" section).

3. **Pick the next `not_started` phase-7 feature** from `feature_list.json`. Likely candidates in dependency order: `feat-040` (runtime×mode validation matrix), `feat-041` (per-turn `TurnContext`), or `feat-042` (per-adapter model cache — the 501 branch on `list_provider_models` for `kind=cli` rows added in feat-039 lands its first user here).

4. **Set the chosen feature to `active`** and proceed with the standard 7-phase feature-dev workflow (`/feature-dev:feature-dev start feat-NNN`).

5. **Open items carried forward from previous sessions (low priority, log not fix):** the untracked `weave.db.bak.20260609-*` backup files at the repo root are still there. Confirm they can be deleted, then `rm weave.db.bak.20260609-110204 weave.db.bak.20260609-160418`.

4. **Restart the dev server cleanly.** The new weave-server is running (pid 48273 area, uptime ~3 min at session end), but `cargo watch` (pids 19633/19634) is still watching the repo. After fix-067 is committed, the cargo watch will rebuild on the next source change. If you want a clean restart from scratch, kill the cargo watch shim and the server, then `just dev` (or `just dev-web` for the Vite side).

5. **Out-of-scope items noticed (logged in fix-068 entry, not fixed):** none new this session. The pre-existing `type_complexity` clippy warning in `service/sessions.rs:1436` (test helper) is unchanged — it doesn't fail `just lint` because lint runs without `--all-targets`.

## Session End Verification (2026-06-09, end of fix-068 commit)

Working tree state at session end matches the "Next Steps" checklist above:

- `git status --short` shows the 5 fix-067 files modified and 1 new (`web/src/app/__tests__/session-traces.test.tsx`) — these are the work the next session will pick up.
- `weave.db.bak.20260609-160418` is untracked at the repo root (the pre-`BEGIN IMMEDIATE` byte-for-byte copy from the 18-session recovery). Item 3 in the Next Steps above covers deletion.
- `crates/weave-server/src/service/startup.rs` and the fix-068 PROGRESS.md section are committed at `81ce146` (docs) and `1cd4ab7` (fix).
- Server is running on `:3000` (uptime ~10 min at session end), Vite on `:5173`. The dev server will auto-rebuild on the next source change via `cargo watch` (pids 19633/19634).
- `./init.sh` last passed at session end (clippy, ESLint, fmt, Prettier, Rust tests, frontend tests, build, `/api/health` smoke).

### feat-062 — Attach codebase to session (committed; manual smoke by user)

Attach a registered codebase (git repo) to a session at creation time. The session's `cwd` is the codebase's `path`; the FS-tool sandbox (fs_read/fs_list/fs_search + the explicit-cwd form of shell_exec/git_*) is contained within the repo, and the FS walkers deliberately do NOT follow symlinks (so `ln -s /etc <repo>/etc_link` cannot be used to escape).

**What's in the working tree:**
- New migration `010_session_codebase_id.sql` — `codebase_id TEXT REFERENCES codebases(id) ON DELETE SET NULL` + index
- `Session.codebase_id: Option<String>` plumbed through store/api/service/migration
- `CreateSessionRequest.codebase_id: Option<String>` — server resolves to codebase's path, copies onto `cwd` (binding wins over any supplied `cwd`); cross-workspace ids rejected with `AppError::NotFound`
- `ToolContext.codebase_root` collapses to `session.cwd` when bound, `.` when unbound (47-line over-engineered SQL path removed in review)
- `validate_read_path` helper in `tools/fs/mod.rs` (sibling to `validate_write_path`); called by fs_read/fs_list/fs_search + the explicit-cwd form of shell/git
- FS walkers in `fs/list.rs` and `fs/search.rs` use `entry.file_type()` and skip symlinks
- Frontend: `Session.codebase_id: string | null`; `NewSessionModal` adds a "Codebase" dropdown with disabled empty-state + /codebases link; `app/pages/session.tsx` shows a monospace pill with the codebase basename
- Docs: `docs/user/sessions.md` adds "How sessions use a codebase" section; `docs/user/codebases.md` rewrites the same section; both state the dual claim (cwd-arg containment yes, shell-body jail no)
- 5 new Rust tests (2 store, 3 service), 2 new frontend tests, all green

**Blocker / Next steps for the next session:**
1. **User runs `./init.sh`** for the system-layer smoke (Layer 3 — `/api/health` + `curl / | grep 'id="root"'`). If green, the next session should:
   - Open the dev server with `just dev` and `just dev-web`
   - With agent-browser: create a workspace, register a codebase, create a session bound to that codebase, verify the session header shows the path pill, verify the agent's `fs_read` of a path outside the repo is rejected with the new "outside the codebase root" error
   - Promote `feat-062` in `feature_list.json` from `state: "active"` to `state: "passing"` with the `./init.sh` output and the agent-browser observation in `evidence`
2. The simplify review surfaced 3 lower-priority items deferred from this slice:
   - `validate_read_path` / `validate_write_path` share canonicalize+starts_with — could extract a private helper
   - Test-fixture sprawl (30+ extra `None` args on `SessionStore::create` / `create_tx` / `SessionService::create_session`) — add a `create_session_basic` test helper, or convert the API to a `CreateSession { ... }` builder struct
   - No direct unit tests for `validate_read_path` — the unbound (`codebase_root == "."`) branch is not exercised by any current test
3. Resume does NOT auto-inherit the parent's `codebase_id` — design choice, but worth a follow-up: when resuming a bound parent, default the new session's codebase picker to the parent's codebase (or pass it server-side).
4. Pre-existing `tools/fs/mod.rs:167-217` `resolve_path` bug for deeply non-existent files (drops the file name, duplicates the last tail component). Unrelated to this slice; flagged in review.
5. Kanban auto-spawn in `service/kanban.rs:130` still passes `codebase_id: None`; the `tasks` model has no `codebase_id`. Wiring kanban is a separate feature.

### feat-063 — `/codebases` and `/boards` empty-state fix + modal extract (uncommitted; both halves done)

Drove agent-browser through every workspace-related surface (Home, `/workspaces/:id`, `/sessions`, `/boards`, `/codebases`, `/settings`, New Session modal) and found three functional gaps. The first two are fixed and verified; the third is queued for a future session.

**`/codebases` fix (in working tree, uncommitted):**
- The pre-fix `WorkspaceCodebases` in `codebases.tsx:30-31` returned `null` when `codebases.length === 0`, leaving the page with no entry point to register the first codebase. Same anti-pattern that feat-061 just fixed in `/sessions`.
- New `web/src/components/new-codebase-modal.tsx` (182 lines) — extracted from the inline `CreateCodebaseModal` in `codebases.tsx`. Mirrors `new-session-modal.tsx` shape exactly: `{ workspaceId: string | null; onClose, onCreated?: (codebase: Codebase) => void }`, inline `role="alert"` error, `useEffect` form-reset on every open transition, `FIELD_CLASS`/`LABEL_CLASS` constants, `useCreateCodebase` hook.
- `codebases.tsx` rewritten: `WorkspaceCodebases` now always renders the workspace heading + `+ New codebase in {name}` button (right-aligned in the header row, matching post-feat-061 `sessions.tsx`). Empty state is an inline `<p>No codebases yet</p>` in place of the list. On successful create, navigates to `/workspaces/:wid/codebases/:cid`.
- `__tests__/codebases.test.tsx` flipped: the old "does not render a workspace section when its codebase list is empty" test is now the positive "renders the workspace heading and + New codebase button even when the codebase list is empty" (asserts heading, button, and "No codebases yet" copy all present). Added 2 new tests mirroring `sessions.test.tsx`: click-the-per-workspace-button-opens-NewCodebaseModal and submit-creates-codebase-and-navigates. 9 tests pass (was 7).

**`/boards` fix (in working tree, uncommitted; 1-to-1 port of the /codebases fix):**
- The pre-fix `WorkspaceBoards` in `boards.tsx:30` had `if (error || boards.length === 0) return null;` — the identical anti-pattern.
- New `web/src/components/new-board-modal.tsx` — extracted from the inline `CreateBoardModal` in `boards.tsx`. Same contract as `new-codebase-modal.tsx`: `{ workspaceId, onClose, onCreated?: (board: Board) => void }`, inline `role="alert"` error, `useEffect` form-reset on open, `FIELD_CLASS`/`LABEL_CLASS` constants. Uses a new `useCreateBoard(workspaceId)` hook added to `web/src/hooks/use-board.ts` (mirrors `useCreateCodebase` shape: `useMutation` + `invalidateQueries` on success).
- `boards.tsx` rewritten: dropped the local `bannerError` state + `ErrorBanner` import (the modal owns its own inline error). `WorkspaceBoards` now always renders the workspace heading + `+ New board in {name}` button (right-aligned, same shape as `/sessions` and `/codebases`). Empty state is an inline `<p>No boards yet</p>`. On successful create, navigates to `/workspaces/:wid/boards/:bid`.
- New `__tests__/boards.test.tsx` (6 cases, mirroring `codebases.test.tsx`): no-workspaces empty state, workspace heading + button visible when boards empty (the bug fix), rows + button coexist, click button opens the NewBoardModal, submit creates board and navigates, create button is disabled when name is empty.
- `./init.sh` all 3 layers green (98 frontend tests pass, was 90; +8 for feat-063: 2 for /codebases, 6 for /boards).
- agent-browser verified both /boards states end-to-end:
  - **Empty:** deleted both boards via API, reloaded, the page shows the workspace heading + `+ New board in default` button + `<p>No boards yet</p>` (the bug fix). Pre-fix, the whole block returned null and there was no entry point.
  - **Create flow:** clicked the button, the modal opens with "New Board" heading + disabled "Create Board" submit + empty placeholder. Typed "My Sprint Board Real" via `keyboard inserttext` (after native value setter), the submit button enabled. Clicked submit, the modal closed, the URL navigated to `/workspaces/5a7675ff.../boards/0624af02...` and the board detail page rendered the new board's name as the h1. Cancel button closes the modal cleanly.

**Blocker / Next steps for the next session:**
1. **Commit the 7 in-tree files** (2 new modals, 2 rewritten pages, 1 hook addition, 2 test files). One commit is fine since both halves are the same fix: `fix: /codebases and /boards always show heading + create button on empty (mirrors feat-061)`. The commit body should reference feat-061 as the precedent and call out the 8 new tests + agent-browser evidence.
2. **Promote feat-063 in `feature_list.json`** — no entry exists for this yet (it was treated as a follow-up, not a numbered feat). Decide whether to backfill a `feat-063` entry or just commit the work as a post-feat-061 follow-up under a single commit. If backfilling, copy the structure of the `feat-061` entry.
3. **Other workspace-UI gaps surfaced by the agent-browser walkthrough but out of scope for feat-063** (logged in case they get picked up later):
   - `/workspaces/:id/sessions` and `/workspaces/:id/settings` return 404 — there is no per-workspace sessions or settings route. The Settings page at `/settings` is top-level and lists all providers globally (the workspace detail page has no settings link to go to).
   - Workspace detail page (`workspace.tsx`) has no Rename/Delete actions, no link to per-workspace boards/codebases/specialists, no workspace metadata (status, created/updated, last-activity), no filter/search/pagination on the 17-row session table, no session actions from the list (delete/archive/fork).
   - Sessions list has the same em-dash / no-specialist sparseness as the workspace table.
   - New Session modal: Specialist dropdown shows 5 names with no descriptions (YAML `description` frontmatter not surfaced), Model is a free-text input with no autocomplete from the provider's known models.
   - Settings page: "Type" field is a non-editable-looking "Anthropic" label (no select for multi-type), Providers table ACTIONS column is empty (no edit/delete/test).
   - Sidebar has no workspace switcher, no global search, no notifications.
4. **Pre-existing de-dup follow-ups (from feat-061, still pending):** the per-workspace `+ New {entity} in {name}` button is now triplicated across sessions/boards/codebases (extract `<PerWorkspaceCreateButton>`); the X close-icon SVG in the modal header is in 7 places (extract `<CloseButton>` or `<ModalHeader>`); the form input/label class strings are duplicated 13+ times (extract `web/src/lib/form-classes.ts`); the test-render QueryClient+MemoryRouter boilerplate is now in 5 places (extract `web/src/__tests__/test-render.tsx`).

### fix: New Session modal — inline codebase creation (uncommitted; this session)

User bug report: opening the New Session modal in a workspace with no codebases shows a disabled dropdown and a `<Link to={ROUTES.codebases}>` saying "Register a codebase" — the user has to navigate away to register one, losing the session-creation flow. Discovered via agent-browser (PROGRESS.md session: opened `/sessions`, clicked `+ New Session in default`, snapshot showed the disabled dropdown + navigation link).

**Three changes (4 files):**

1. `web/src/components/modal.tsx` — added two optional props: `closeOnEscape?: boolean` (default `true`, new use: ignore Escape when a nested modal is open) and `zIndex?: number` (default `50`, replaces the hard-coded `z-50` class via inline `style`). Both are backward-compatible; the 4 existing Modal callers (NewSessionModal, NewCodebaseModal, NewBoardModal, AddCardModal, AddColumnModal, settings) are unaffected.

2. `web/src/components/new-codebase-modal.tsx` — accepts the new `zIndex` prop and forwards it to its internal `<Modal>`, so the NewSessionModal can pass `zIndex={60}` to stack the inner modal above the outer's backdrop.

3. `web/src/components/new-session-modal.tsx`:
   - The "Register a codebase" `<Link to={ROUTES.codebases}>` becomes a `<button onClick={() => setShowNewCodebase(true)}>` that opens a nested `<NewCodebaseModal>`.
   - The outer `<Modal>` gets `closeOnEscape={!showNewCodebase}` so Escape closes the inner first.
   - On successful codebase create, `onCreated={(cb) => setCodebaseId(cb.id)}` auto-selects the new codebase in the dropdown.
   - **Bug fix surfaced during verification:** the consumer was doing `const codebases = codebasesResp?.data ?? [];` — but `api.codebases.list` returns `Codebase[]` directly (the `apiFetch` helper unwraps the `{data: T}` envelope), so `codebasesResp?.data` is always `undefined` in production. The dropdown never populated after a successful create. Changed to `const codebases = codebasesResp ?? [];`. The unit tests passed against the wrong mock format (`{ data: mockCodebases }`) and didn't catch this — the mock was the only thing that matched the buggy consumer. Tests now mock the unwrapped array.

4. `web/src/app/__tests__/sessions.test.tsx`:
   - Flipped the existing `codebases list > the codebase picker shows a disabled empty state with a /codebases link` test → button (same regex matches the new copy; assertion now checks for a button, not a link).
   - Added a new test: click "Register a codebase" → nested NewCodebaseModal opens (asserts both "New Codebase" and "New Session" headings are present) → fill path + submit → mutation fires with the right payload → inner modal closes → outer stays open → dropdown is populated and the new codebase is the selected value.
   - Updated all `mockApi.codebases.list.mockResolvedValue*` calls to return the unwrapped array (matches production).

**Verification:**
- `bun run test` → 99/99 frontend tests pass (was 98; +1 new test, 0 regressions).
- `bun run lint` → clean. `bun run format:check` → clean.
- agent-browser end-to-end: opened `/sessions`, clicked `+ New Session in fresh-test` (a workspace with 0 codebases), modal opened with the empty-state branch + Register button, clicked Register → nested NewCodebaseModal opened, filled `/tmp` + Create Codebase, inner modal closed, outer stayed open, CODEBASE dropdown now shows `/tmp` as the selected value. Pre-fix this exact flow ended with the dropdown still showing "No codebases registered" (the data-shape bug from #3 above).
- Pre-existing typecheck error in `node_modules/@types/estree` (ArrowFunctionExpression body type mismatch) is unrelated to this fix — confirmed by stashing the changes and re-running.

**Blocker / Next steps for the next session:**
1. **Commit the 4 in-tree files** as a single fix: `fix: New Session modal — inline codebase creation`. Body should reference the feat-062 / feat-063 lineage and call out the 1 new test, 1 flipped test, and the 3 mocks re-formatted. Mention the Modal prop additions as the foundation for future nested-modal flows.
2. **The DELETE codebase endpoint is not implemented** (verified via `curl -X DELETE → 405 Method Not Allowed`). Discovered while trying to reset the default workspace for the verification run; not in scope for this fix but worth a follow-up. Until it lands, the only way to remove a codebase is to wipe the DB.
3. **Pre-existing de-dup follow-ups** from feat-061 still apply (now with one more occurrence of the per-workspace button and modal form-class strings).

## Completed Since Project Start

## Completed Since Project Start

- [x] System design docs (`docs/SYSTEM_DESIGN.md`, `docs/ARCHITECTURE.md`, `docs/road-map/PLAN.md`)
- [x] **feat-001**: Binary skeleton (CLI, tracing, health check, graceful shutdown)
- [x] **feat-002**: SQLite with WAL mode, migrations (11 tables)
- [x] **feat-003**: Shared error types (AppError, ProviderError)
- [x] **feat-004**: Workspace CRUD (store + API + default seed)
- [x] **feat-005**: CodingAgent trait (StreamEvent, StopReason, Send+Sync)
- [x] **feat-006**: AnthropicAgent (SSE streaming, error mapping, retry)
- [x] **feat-007**: ProviderStore + ProviderRegistry (CRUD, api_key stripping)
- [x] **feat-008**: SessionStore + MessageStore (state machine, pagination)
- [x] **feat-009**: SessionService (prompt lifecycle, streaming, cancellation)
- [x] **feat-010**: SSE infrastructure (SseManager, EventBuffer, reconnection)
- [x] **feat-011**: SpecialistLoader (YAML frontmatter, system prompt injection)
- [x] **feat-012**: ToolRegistry (ToolExecutor trait, 5 profiles)
- [x] **feat-013**: Filesystem tools (fs_read/write/edit/search/list, PathValidator)
- [x] **feat-014**: Shell tool (shell_exec, timeout, 100KB truncation)
- [x] **feat-015**: Git tools (status, diff, log, commit, identity validation)
- [x] **feat-016**: Task context tools (get/list/update, workspace-scoped)
- [x] **feat-017**: TraceCollector (channel-based, background flush, 3 API endpoints)
- [x] **feat-018**: Session resume (parent chain, depth limit 5, cycle detection)
- [x] **feat-019**: React frontend scaffolding (Vite + React 19 + TS + Tailwind + TanStack Query)
- [x] **feat-020**: Frontend pages (Home, Workspace, Settings, shared components)
- [x] **feat-021**: Session chat view (useSession hook, SSE streaming, Markdown)
- [x] **feat-022**: Journey sidebar (Decision timeline, FileChangesList, collapsible)
- [x] **feat-023**: Frontend served from Rust binary (build.rs + ServeDir fallback)
- [x] **feat-024**: KanbanStore CRUD + 8-endpoint HTTP API (boards, columns, tasks)
- [x] **feat-025**: KanbanService lane automation + board-scoped SSE stream
- [x] **feat-026**: Kanban frontend (@dnd-kit, real-time SSE, TaskDetailPanel)
- [x] **feat-027**: Default board template + 5 built-in specialists
- [x] **feat-028**: Kanban tools for agents (get_board, move_card, create_card, search_cards) + transition gates
- [x] **feat-029**: A2A protocol server endpoints (Agent Card, SendMessage, GetTask, CancelTask, SubscribeToTask SSE)
- [x] **feat-030**: Note tools for agents (create, read, list, set_content, append)
- [x] **feat-031**: Artifact tools (request, provide, list) + kanban transition gate-3
- [x] **feat-032**: CodebaseStore + API + frontend pages
- [x] **feat-033**: Enhanced health check (version, uptime, provider total/healthy/unhealthy, per-workspace active_sessions, db size_bytes, wal_checkpoint_pending; 10s provider-health TTL cache; always 200 with status="ok"|"degraded")
- [x] **feat-034**: Graceful shutdown — SIGTERM/SIGINT/drain-cap race, parent CancellationToken in AppState, ActiveSessions::cancel_all, SseWireEvent::Shutdown + SseManager::broadcast_shutdown, Db::checkpoint (TRUNCATE), service::startup::reap_orphans (transactional mark-as-error), spawn cleanup task, run() extracted from main(). 12 new tests.
- [x] **feat-036**: Session chat re-implementation (message_persisted SSE, useReducer, id-based handoff)
- [x] **feat-037**: Native Anthropic tool-execution loop (agent_loop, ToolOutcome, JSON Schema validation, sanitize_tool_input, EventConverter deferred-emit, LoopLimit stop_reason). 7 spec tests cover basic happy path, unknown tool, validation error, exec error, loop limit, cancel mid-loop, and no-tool passthrough.
- [x] **feat-061**: `+ New Session` button on `web/src/app/pages/sessions.tsx`. Extracted the inline New Session modal from `workspace.tsx` into `web/src/components/new-session-modal.tsx` (Provider select + Specialist dropdown via `useSpecialists` + Model input + inline `role="alert"` error, contract `{ workspaceId: string | null, onClose, onCreated? }` matching `CreateBoardModal` precedent). Refactored `workspace.tsx` to use the new component (page shrank 344 → 220 lines, ~124 net lines removed). Added a per-workspace `+ New Session in {name}` button to `sessions.tsx` (slate-secondary style matching boards/codebases) that opens the shared modal pre-bound to that workspace. Restructured `WorkspaceSessions` so a workspace with zero sessions still shows the heading + button (per the user's "show heading+button on empty" requirement). Updated `docs/user/sessions.md` to say "next to the workspace name" (placement) and to describe the specialist as a dropdown. 5 new page tests in `__tests__/sessions.test.tsx`. **Spec deviation**: the spec said "render the modal once per `WorkspaceSessions` block"; the implementation uses one shared modal at page level (matches boards/codebases precedent, TanStack Query dedupes the providers/specialists queries, the page-level modal control state is simpler).

## In Progress

(none — all features in phases 1-5 + phase-6 + feat-061 are passing)

## Blocked

(none)

## Remaining Features

| ID | Description | Dependencies |
|----|-------------|-------------|
| feat-035 | Configuration (env vars, CLI, TOML) | feat-001 |
| feat-037 | Native Anthropic tool-execution loop (prerequisite) | feat-005, 006, 009, 012, 013 |
| feat-038 | Session table migration for runtime/mode/cli_resume_id | feat-008 |
| feat-039 | Provider table config discriminated union (HTTP vs CLI) | feat-007 |
| feat-040 | Runtime Tool × mode compatibility validator | feat-005, 038, 039 |
| feat-041 | CodingAgent trait extension for CLI turn context (`TurnContext`) | feat-005, 009, 038 |
| feat-042 | ProviderRegistry model cache (per-Runtime-Tool, 5min TTL) | feat-005, 007, 039 |
| feat-043 | Per-turn CLI subprocess runner | feat-009, 041 |
| feat-044 | Fake CLI test harness (conformance fixture) | — |
| feat-045 | Claude Code `stream-json` parser | feat-005 |
| feat-046 | `PermissionMapper` trait + Claude Code implementation | feat-005, 012, 040, 041 |
| feat-047 | CLI resume metadata persistence + replay fallback | feat-005, 008, 038, 041, 043, 045 |
| feat-048 | `JourneyTranslator` for CLI streams (no re-execution) | feat-005, 017, 043, 045 |
| feat-049 | Child-process reaping on startup + per-session tracking | feat-009, 034, 043 |
| feat-050 | Workspace-scoped CLI session validation (cwd inside codebase) | feat-008, 032, 040 |
| feat-051 | `ClaudeCodeCodingAgent` end-to-end (fake harness) | feat-037…050 |
| feat-052 | Settings page Runtime Tool-aware form | feat-020, 039, 042 |
| feat-053 | 4-step session creation sheet (Runtime Tool → Role → Model → What it works on) | feat-021, 040, 041, 042 |
| feat-054 | Session page layout switcher (native / wrapped / attended) | feat-021, 040, 051 |
| feat-055 | Kanban column `(runtime_kind, specialist_id)` binding | feat-024, 025, 040 |
| feat-056 | A2A explicit Runtime Tool selection (no first-provider fallback) | feat-029, 040 |
| feat-057 | Shared CLI adapter conformance test suite | feat-043, 044, 045, 046, 047, 048, 050 |
| feat-058 | `CodexCodingAgent` adapter | feat-051, 057 |
| feat-059 | `OpenCodeCodingAgent` adapter | feat-051, 057 |
| feat-060 | Attended mode `Terminal` abstraction (deferred) | feat-051 |

Detailed task descriptions (per-feature engineering handoff) live at `docs/road-map/multi-runtime-tasks.md`.

## Key Architectural Decisions

See `DECISIONS.md` for full rationale. Quick reference:
- Single Rust binary with embedded frontend (build.rs)
- SQLite with WAL mode, no ORM (raw rusqlite)
- SSE for all real-time (no WebSocket)
- Workspace-scoped resources (every query includes workspace_id)
- `feature_list.json` is single source of truth for task scope

## Out-of-Scope Items Noticed

Items deferred from past sessions. Address when a feature touches the relevant area.

- **`verify_task_in_workspace` duplicated** across `store/artifacts.rs`, `service/kanban.rs`, `api/kanban.rs` — 3 copies of "look up task's workspace via board". Fix: add `TaskStore::workspace_id_for_task`.
- **`seed_task` helper duplicated** across 5+ tool test files. Fix: add to `kanban_test_helpers.rs`.
- **Unmatched `/api/*` paths return index.html** instead of 404 JSON (feat-023 fallback catches them). Fix: nest API router under `/api` with JSON 404 handler.
- **`SseManager` channel GC**: no cleanup for stale board/session channels on long-running servers.
- **Transition gates bypassed on HTTP PATCH**: `api/kanban.rs::update_task` calls `move_to_column` without `check_transition_gates`. Frontend drag-and-drop bypasses the gate.
- **TOCTOU between gate check and move**: gate runs in a read tx, move in a write tx. Window is tight (SQLite WAL serializes) but exists.
- **`MAX_TASK_TITLE_LEN` defined in two places**: `tools/fs/mod.rs` and `api/kanban.rs`. Fix: hoist to `store::tasks`.
- **Cancel button always visible** in session header even when no stream is active. UX wart.
- **Tool-containment gap** (security audit, feat-037 review): `ToolContext.codebase_root` is hardcoded to server CWD (`service/sessions.rs:436`). `fs_read` (`tools/fs/read.rs:34-60`), `fs_list` (`tools/fs/list.rs:47`), and `fs_search` (`tools/fs/search.rs:55-59`) only call `PathValidator::require_absolute` — they do NOT call `validate_write_path`, so a model can read or list any absolute path the server can reach. `shell_exec` (`tools/shell.rs:63-77`) does not validate `cwd` against `codebase_root` either. Fix in a future feature: add `root_path` to `workspaces` table; require every tool path arg to be contained under `codebase_root`.
- **Tool `input_schema` compile failure silently allows the call** (`service/sessions.rs:692-702`). Should return `ValidationFailed` instead of proceeding.
- **`tracing::debug!(... command = %command ...)` in `shell_exec`** (`tools/shell.rs:82-88`) logs the full shell command including any embedded secrets. Drop the `command` field, keep only binary name + arg count.
- **`agent_loop` clones `history` and `tool_defs` per iteration** (O(n²)). Switch `MessageRequest` to borrow `&[Message]` + `&[ToolDefinition]`.
- **`SHUTDOWN_DRAIN_CAP = 30s` always fires in dev** — **FIXED in 2026-06-05 UI-validation session** (`crates/weave-server/src/main.rs`). Replaced the hard-coded 30s const with a `WEAVE_SHUTDOWN_DRAIN_CAP_SECS` env var (unset / `0` / unparseable → `None` = no cap, the new dev default). `shutdown_signal_with_cap` now takes `Option<Duration>` and skips the cap branch entirely when `None`. 611 tests still pass; live cargo watch run kept the server up past 30s with no env var set. CI / orchestrators that want a bound set the env var explicitly. Doc-comments on the cap and on the helper were rewritten to match the new semantics.
- **`+ New Session` button missing on Sessions list page** (`web/src/app/pages/sessions.tsx:69-86`). `docs/user/sessions.md` (lines 30-31) tells the user to "from the `Sessions` list, click `+ New Session` in the page header" — but the page renders only a heading, subtitle, and per-workspace session lists. There is no header action. The create-session entry point only exists on the workspace detail page (`workspace.tsx`, "New Session" button at `ref=e4`), reachable via the back link from a session or via Home → workspace. Either add the button to `sessions.tsx` (needs a `workspace_id` from somewhere — currently the page iterates over all workspaces, so it'd need to be a per-workspace action on each `WorkspaceSessions` block, or a top-level dropdown) or amend the doc to drop the Sessions-list path. UI-validation 2026-06-05 confirmed.

## Session Notes

### 2026-06-03 — feat-029, feat-030, feat-031, feat-032
- feat-029: A2A protocol implemented (6 files in `src/a2a/`, migration 009 adds `context_id` to sessions). 582 Rust tests.
- feat-030: Note tools (5 tool executors, `notes` table via migration 008). `map_insert_error` hoisted to `db.rs` (3rd caller). 569 Rust tests.
- feat-031 Phase 6 reconciliation: all 8 critical+important review fixes confirmed already-applied. PROGRESS.md updated.
- feat-032: CodebaseStore + API + frontend (4 new backend files, 4 new frontend files). 518 Rust tests + 83 frontend tests.

### 2026-06-04 — feat-033
- Enhanced health check (`GET /api/health`): added `providers` (total/healthy/unhealthy), `active_sessions` (per-workspace `BTreeMap`), `database` (size_bytes, wal_checkpoint_pending, reachable). Raw JSON shape preserved (liveness-probe contract). Provider health probed in parallel via `futures_util::future::join_all` with a 10s TTL cache; `add_agent`/`remove_agent` invalidate the cache. `degraded` rule: `healthy == 0 || !database.reachable`. 593 Rust tests pass (11 new). 4 files touched: `db.rs` (+ `path: PathBuf`, `size_bytes`, `wal_checkpoint_pending`), `store/sessions.rs` (+ `count_active_by_workspace` using the `TERMINAL` const), `agent/registry.rs` (+ `health_cache`, `cached_health_summary`, `agents_snapshot`, `invalidate_health_cache`), `api/health.rs` (rewrote `HealthResponse`, added `ProviderSummary`/`DatabaseInfo` and 4 integration tests including a cache-hit + healthy-status pair).

### 2026-06-02 — feat-022, feat-026, feat-023
- feat-022: Journey sidebar. Backend SQL filter tightened to Decision+Error only. Frontend: 5 components, 14 new tests.
- feat-026: Kanban frontend. @dnd-kit drag-and-drop, SSE real-time updates, TaskDetailPanel slide-over. 17 new tests.
- feat-023: Frontend served from Rust binary. First `build.rs`, `static_assets.rs` with SPA fallback. 5 new tests.
- Bug fix: Journey sidebar decision fragmentation (177 rows → ~5 per turn). Thinking deltas coalesced into single Decision per reasoning pass.

### 2026-06-01 — feat-019, feat-020, feat-021, feat-036, bug fixes
- Frontend scaffolding + pages + session chat view implemented.
- feat-036: Session chat re-implementation (message_persisted SSE, useReducer, id-based handoff).
- Multiple bug fixes: session terminated after first turn, message ordering by UUID, user message invisible, page flash on completion, stale "Thinking..." badge.

### 2026-05-31 — Initial harness + feats 001-018
- Core foundation: binary, database, providers, sessions, SSE.
- Agent tools: filesystem, shell, git, task context, TraceCollector.
- Session resume with parent chain validation.

### 2026-06-04 — User-facing docs under `docs/user/`
- Created `docs/user/` mirroring routa's `use-routa/` style: short, scannable, second-person, UX-focused (not internals).
- 11 files: `index.md` (landing), `quickstart.md` (5-min path), then one per feature (workspaces, providers, sessions, journey, kanban, codebases, specialists), plus `common-workflows.md` and `best-practices.md`.
- Internal `docs/` (ARCHITECTURE, data-model, etc.) stays the engineer-facing source of truth; `docs/user/` is the user-facing counterpart and the right handoff for new Weave users.
- No code changes, all 605 Rust + 83 frontend tests still green, `./init.sh` still passes.

### 2026-06-04 — Multi-runtime strategic plan
- Wrote `docs/road-map/multi-runtime-strategy.md` (committed strategic direction). Commits the direction: sessions gain a Runtime Tool axis (`claude-code` / `codex` / `opencode` / `anthropic-api` / `openai-api` / `openai-compatible`) and a `mode` (`native` / `wrapped` / `attended`) axis. The first implementation prerequisite is the native Anthropic tool-execution loop; Claude Code CLI wrapped mode is the first CLI target. The `Provider` table widens to a discriminated union; `CliCodingAgent` is added alongside `AnthropicAgent` with request/context shape to revisit; attended mode is a separate `Terminal` abstraction.
- Records the non-obvious calls: Claude Code CLI wrapped mode is the first implementation target, specialists stay prompt-only, models come from the tool not Weave, journey is the unifying artifact, per-turn subprocess for wrapped mode, the `Multiple concurrent providers` drop in `SYSTEM_DESIGN.md` is amended.
- Registered in `docs/SYSTEM_DESIGN.md` routing map. Pointer in `DECISIONS.md` (2026-06-04 entry). Doc-only change — no code, no schema migration, no API surface change yet.
- Implementation plan is the next deliverable; the strategic plan explicitly defers schema, API, and frontend decisions to it.

### 2026-06-04 — Multi-runtime task breakdown
- Broke the strategy into 24 implementation features across 6 new phases in `feature_list.json` (feat-037…feat-060). All new entries `state: "not_started"`. WIP=1 invariant preserved (no feature in `active` state). Existing 35 passing features and feat-035 (not_started) untouched.
- Phases: phase-6 (native tool loop), phase-7 (multi-runtime foundation: schema + trait + cache), phase-8 (Claude Code wrapped mode — 9 features), phase-9 (multi-runtime user surface), phase-10 (Codex/OpenCode adapters), phase-11 (attended mode, deferred).
- Key commitments baked into the breakdown: `TurnContext` extends the `CodingAgent` trait (not `MessageRequest`); `cli_resume_id` lives inside `runtime_metadata_json` (generic per-runtime column, not CLI-specific); `attended` mode is rejected at session creation until Phase 11; adapter conformance suite (feat-057) is a hard gate for Codex/OpenCode.
- Detailed per-feature task descriptions (engineering handoff format) live at `docs/road-map/multi-runtime-tasks.md` (created in this session).
- `feature_list.json` validated: 11 phases, 60 features, all phase refs resolve, all dependency targets exist, states preserved. JSON load test passed.

### 2026-06-05 — UI validation session (`docs/user/sessions.md` walkthrough)
- Discovered runtime bug: `SHUTDOWN_DRAIN_CAP = 30s` (feat-034) always fired in dev (no TTY), so `just dev` restarted the server every 30s. **Fixed in `84a5621`**: cap is now opt-in via `WEAVE_SHUTDOWN_DRAIN_CAP_SECS` env var (unset = no cap = new dev default). `shutdown_signal_with_cap` takes `Option<Duration>` and skips the cap branch when `None`. 611 tests still pass.
- Walked `docs/user/sessions.md` end-to-end via agent-browser at `http://localhost:5173/`. Found one real doc/UI gap: **"+ New Session" button missing on `web/src/app/pages/sessions.tsx`** — the doc says it's in the page header; the page only renders a heading and per-workspace session lists. Create entry point exists only on `workspace.tsx`. Logged as `feat-061` in `feature_list.json` (phase-3, deps: feat-020) for pickup via /feature-dev. Other doc claims verified ✓.
- No regressions observed. Decision fragmentation visible in Journey sidebar is historical (sessions dated 6/1 predating the 6/2 feat-022 coalesce fix); no post-fix data to test against.

### 2026-06-05 — feat-061 (+ New Session button on /sessions)
- Implemented via /feature-dev workflow. Extracted `web/src/components/new-session-modal.tsx` from the inline modal in `workspace.tsx`; refactored `workspace.tsx` to use it (page shrank 344 → 220 lines, removed `useProviders`/`useCreateSession`/`Modal`/`ErrorBanner` imports and ~100 lines of form/modal/state). Added per-workspace `+ New Session in {name}` button to `sessions.tsx`; restructured `WorkspaceSessions` so a workspace with zero sessions still shows the heading + button (a deliberate divergence from boards/codebases which still hide on empty — logged as a follow-up). Specialist input upgraded from free text to `<select>` populated by `useSpecialists()`. Updated `docs/user/sessions.md:30-31, 34-36` to match. 5 new page tests in `__tests__/sessions.test.tsx` cover: no-workspaces empty state, per-workspace button visible on zero sessions, session rows + button coexist, click button opens modal, submit creates session and navigates to `/sessions/:id`. `./init.sh` all 3 layers green. Simplify pass extracted `FIELD_CLASS`/`LABEL_CLASS` constants and removed a redundant `setCreateWorkspaceId(null)` (modal already calls `onClose()` first). 611 Rust + 88 frontend tests pass.
- Follow-ups logged (out of scope for this PR): the per-workspace `+ New {entity} in {name}` button is now triplicated across sessions/boards/codebases (extract `<PerWorkspaceCreateButton>`); the X close-icon SVG is now in 7 places (extract `<CloseButton>` or `<ModalHeader>`); the form input/label/button class strings are duplicated 13+ times across all forms (extract `web/src/lib/form-classes.ts`); the test-render QueryClient+MemoryRouter boilerplate is the 5th copy (extract `web/src/__tests__/test-render.tsx`); boards/codebases still hide the per-workspace section when empty (the new sessions.tsx pattern should be ported — extract `<WorkspaceListSection>` to enforce the invariant once); `workspace.tsx` page has no test (pre-existing coverage gap).

### 2026-06-09 — feat-063 (/codebases and /boards empty-state fix + modal extract)
- Drove agent-browser through every workspace-related surface at `http://localhost:5173/` (Home, `/workspaces/:id`, `/sessions`, `/boards`, `/codebases`, `/settings`, New Session modal). Found three functional gaps: the `/codebases` and `/boards` empty-state bug (per-workspace block returns `null` on 0 entities — same anti-pattern feat-061 just fixed in `/sessions`); the `/workspaces/:id/sessions` and `/workspaces/:id/settings` 404s (no per-workspace routes exist for sessions or settings).
- **First session:** applied the `/codebases` half of the fix. Extracted `CreateCodebaseModal` to `web/src/components/new-codebase-modal.tsx` (mirroring `new-session-modal.tsx`); refactored `codebases.tsx` so `WorkspaceCodebases` always renders the heading + `+ New codebase in {name}` button (right-aligned in the header row) and shows an inline "No codebases yet" placeholder when the list is empty. On successful create, navigates to `/workspaces/:wid/codebases/:cid`. Updated `__tests__/codebases.test.tsx`: flipped the old "does not render" test to a positive one, added 2 new tests for the click-to-open-modal and submit-and-navigate flows. 92 frontend tests pass. `./init.sh` all 3 layers green. agent-browser verified both states.
- **Second session (this one):** applied the `/boards` half as a 1-to-1 port. Extracted `CreateBoardModal` to `web/src/components/new-board-modal.tsx`; added `useCreateBoard(workspaceId)` to `web/src/hooks/use-board.ts` (mirrors `useCreateCodebase`); refactored `boards.tsx` to always render the heading + `+ New board in {name}` button + inline "No boards yet" placeholder, with inline modal error (dropped the local `bannerError` state and `ErrorBanner` import). New `__tests__/boards.test.tsx` (6 cases, mirroring `codebases.test.tsx`). 98 frontend tests pass (was 92, +6 for boards). `./init.sh` all 3 layers green.
- agent-browser end-to-end on /boards: deleted both boards via API, reloaded, the page shows heading+button+"No boards yet" (the bug fix). Clicked the button, modal opened with disabled submit, typed "My Sprint Board Real" via `keyboard inserttext` (after native value setter), submit enabled, clicked submit, modal closed, URL navigated to `/workspaces/5a7675ff.../boards/0624af02...`, the board detail page rendered the new board's name as the h1. Cancel closes cleanly.
- Uncommitted: 7 files (2 new modals, 2 rewritten pages, 1 hook addition, 2 test files). One commit is fine: `fix: /codebases and /boards always show heading + create button on empty (mirrors feat-061)`. Detailed blocker list at the feat-063 header above.

### 2026-06-09 — fix-066 (Journey sidebar shows tool_call events; regression in feat-037 left all journeys empty)
- **Bug (Phase 1):** On every session, the Journey sidebar's "Decisions & Errors" and "Files" sections always rendered their empty state ("No decisions or errors yet" / "No files touched yet"). User reported it on a single session; investigation showed it was universal — every session, including fresh ones, showed empty Journey. user validation: `agent-browser open http://localhost:5173/sessions/<id>` → toggle sidebar → see only the two empty hints.
- **Root cause (Phase 2):** feat-037 (`ab406e5`) refactored `run_prompt_task` and introduced `agent_loop`, deleting all `trace_collector.emit()` calls in the streaming path except the `Error` arm. A code comment at `service/sessions.rs:2794` acknowledged the regression: "A follow-up feature should either add Decision trace emission to the new loop or remove the sidebar's reliance on it; either way, that work is out of scope for feat-037." The follow-up was never picked up. Why it slipped through: `tests/trace/mod.rs` tests call `collector.emit()` directly (still pass); `test_native_tool_loop_*` tests don't assert trace emission; Journey frontend tests only check empty/loading states; no integration test ran an agent and queried the trace endpoint.
- **Fix part 1 (Phase 3 + 4, backend emission):** In `agent_loop` at `crates/weave-server/src/service/sessions.rs`: (a) added `thinking_buffer: String` cleared per-iteration alongside `turn_text`; (b) added `flush_thinking` helper that emits a `Decision` trace from accumulated thinking at the `TextDelta` / `ToolUseStart` / `Done` / `Error` boundaries (mirrors the pre-feat-037 deleted function); (c) in the tool execution loop, after the `match outcome` block, emit a `ToolCall` trace (`tool_name`, `input`, `output`, `duration_ms`) followed by `extract_file_changes` for any `file_change` traces. Single emission point — matches the pre-feat-037 design. Out of scope: `ToolContext.trace_collector` plumbing is now used but the standalone field could be removed in a follow-up; left as-is to keep the diff small.
- **Fix part 2 (Phase 6, frontend display):** User then reported session `1c6aab4f-...` still showed no Journey data even with the emission fix in place. Investigation: the session had 2 `tool_call` traces in the DB (list_notes, list_tasks), but the Journey sidebar only renders `decision` + `error` (in `useJourney` → `/trace/journey`) and `file_change` (in `useFileChanges` → `/trace/files`). `tool_call` events were recorded but invisible. Root cause for part 2: the Journey sidebar's two-section layout was the wrong design — a session that only used tools (no decisions, no file edits) rendered as fully empty. Fix: added a third "Tools" section. New store method `TraceStore::list_tool_calls` (filters to `event_type = 'tool_call'`); new API handler `get_session_tool_calls` at `GET /api/sessions/{sid}/trace/tools`; new frontend hook `useToolCalls` (TanStack Query wrapper, invalidates on `message_persisted` like its siblings); new `ToolCallsList` + `ToolCallNode` components in `web/src/app/pages/session/journey-sidebar.tsx` that render a chip per tool name (e.g. `list_notes`) with the summary text, time, and a click-to-expand `<pre>` block showing the input + output JSON pretty-printed.
- **Tests:** `test_native_tool_loop_emits_journey_traces` (added in part 1) asserts both `decision` and `tool_call` rows in `TraceStore::list_by_session`, ordering `decision_idx < tool_call_idx`, decision text contains "write the file" (coalesced from 2 Thinking deltas), `list_journey` includes the Decision, `list_file_changes` has the path, and `data_json.tool_name == "fs_write"`. `test_get_session_tool_calls` (added in part 2) inserts mixed events (decision + tool_call + error + tool_call) and asserts the new endpoint returns exactly the 2 tool_call events in timestamp order, with `data_json.tool_name` round-tripping through `insert_batch`. Frontend `journey-view.test.tsx` got 2 new tests: `renders tool_call events from the tools endpoint` (asserts the summary + tool name chip appear) and `expands a tool_call node to reveal input + output JSON` (asserts the `<pre>` block is in the DOM, starts collapsed, expands on click to `max-h-[400px]`).
- **Verified:** 619 Rust tests pass (was 616, +3: 1 part-1 regression test, 1 part-2 backend test, 1 implicit via the `test_native_tool_loop_*` family that the new emission path now exercises). 100 frontend tests pass (was 98, +2 for the two new journey tests). `./init.sh` all 3 layers green. Live agent-browser validation on session `1c6aab4f-...` (the originally-reported session): Journey sidebar now shows "**TOOLS: 2 calls**" with `list_notes (3ms)` and `list_tasks (0ms)` cards, each expandable to show the input/output JSON. Decision and file sections still correctly empty for that session (no decision/error/file events were emitted — model didn't use Thinking or fs_write for that prompt).
- **Out of scope (logged, not fixed):** (1) `ToolContext.trace_collector` is plumbed but each tool execution builds a fresh `TraceCollector` reference rather than the session-scoped one — for this fix the single emission point in `agent_loop` makes the plumbing unused. Future cleanup. (2) The `live test` of part 1 was blocked by the configured model (`MiniMax-M3`) declining to use Thinking for trivial tasks and hallucinating fs_write without actually calling it; the regression test is the load-bearing validation, not the live test. (3) `data_json` for tool_call stores `{ tool_name, input, output, duration_ms }` — the `input` is the parsed JSON (not the raw `input_json` string) so whitespace is preserved as the model emitted it. A future cleanup could add a `tsc`-friendly type for this rather than `Record<string, unknown>`.

### 2026-06-09 — fix-065 (sessions list ordered by last-updated DESC)
- Bug: `http://localhost:5173/sessions` (and `/workspaces/:id`) listed sessions in random order. Root cause: `SessionStore::list_by_workspace` (`crates/weave-server/src/store/sessions.rs:187`) was `ORDER BY id ASC` — UUIDv4 is random, so the visible order was arbitrary. No test pinned the ordering, so the regression-detection surface was empty.
- Fix: changed the SQL to `ORDER BY updated_at DESC, id DESC`. The cursor is now a compound `<updated_at>\x1f<id>` key (keyset pagination), so a single `id` cursor doesn't skip or duplicate rows when consecutive pages straddle a `updated_at` tie. Cursor format is opaque to the client (still a `Option<String>` in the API response).
- Tests added in the same file: `test_session_list_orders_by_updated_at_desc` (the regression test — create two sessions, bump one's `updated_at` via `update_status`, assert the bumped one comes first) and `test_session_list_descending_pagination_is_complete` (creates 5 sessions with distinct `updated_at`, paginates with limit 2, asserts the full set comes back in expected order across all pages).
- Verified: 618 Rust tests pass (was 616, +2 for the new tests). Pre-existing clippy warning in `service/sessions.rs:1340` and 79 pre-existing `tsc` errors are unchanged (both present on `main`). Frontend untouched — `useWorkspaceSessions` just renders what the API returns.
- Out of scope (logged, not fixed): no index on `(workspace_id, updated_at)`. For workspaces with thousands of sessions the sort will spill to a temp file. Add a migration when that becomes a real concern; not blocking the current use.

### 2026-06-04 — Doc reorganization into `docs/road-map/`
- Moved `docs/PLAN.md` and `docs/multi-runtime-strategy.md` into `docs/road-map/`. PLAN moved via `git mv` (rename preserved in history); strategy moved via plain `mv` (was untracked).
- `docs/SYSTEM_DESIGN.md` — added the new doc to the topic-docs routing map; amended the "Multiple concurrent providers" drop to point at the new path. Link targets (relative `(...)`) fixed for both occurrences.
- `CLAUDE.md` — Topic Docs list split into **Road-map** (forward-looking plans) and **Current state** (reference material for the system as it exists). Two new entries in the Road-map subsection.
- `README.md` — Plan link updated to the new path.
- `DECISIONS.md` — multi-runtime strategy link updated (full path retained since DECISIONS.md is at the repo root).
- `PROGRESS.md` — historical journal entries updated to the new paths so future readers can click through.
- Verification: `grep` for the old paths returns empty; `grep` for stale relative link targets returns empty. Doc-only — `./init.sh` is not affected.

## Notes for Next Session

- Package manager is **Bun** (not npm). Use `bun run test`, `bunx vite build`, etc.
- Tailwind CSS v4 uses `@tailwindcss/vite` plugin + `@import "tailwindcss"` (no config file).
- `build.rs` runs `bunx vite build` at compile time. `WEAVE_SKIP_FRONTEND_BUILD=1` to skip.
- Dev: `just dev` (backend) + `just dev-web` (frontend). Production: single binary.
- `./init.sh` is the one-command full verification gate. Run it before and after any change.
- `feature_list.json` is the single source of truth for task scope — do not track work in comments or TODOs.
- The 1 remaining feature is feat-035 (config).
- `docs/user/` is the user-facing documentation set (created 2026-06-04). When a feature ships, consider whether its user-facing guide needs an update — but do not change internal `docs/*.md` from a user-doc session.
