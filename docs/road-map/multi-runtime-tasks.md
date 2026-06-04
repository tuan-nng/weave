# Multi-Runtime Tasks

**Status:** engineering handoff for the strategic plan in [`docs/road-map/multi-runtime-strategy.md`](multi-runtime-strategy.md).
**Scope:** 24 features (feat-037 through feat-060) across 6 new phases. The strategy doc is the source of truth for *what*; this doc is the source of truth for *how to pick up a feature and ship it*.
**Audience:** senior engineers picking up a single feature end-to-end. Every feature in `feature_list.json` has a `behavior` summary; this doc expands each one into a buildable task.

## How to use this doc

- Read the strategy doc first (§3 sequencing + §6 non-obvious calls + §8 resolved policies). The decisions in those sections are *committed* — don't re-derive them.
- Pick the lowest-`active`-state feature whose dependencies are `passing`. WIP=1 is enforced by the session-exit hook.
- Use the **Files** list to navigate; use the **Implementation outline** as a step-by-step build, but treat it as a starting point, not a contract.
- Use the **Acceptance criteria** as the test list. Every bullet should map to at least one test in the verification command.
- Use the **Open decisions** list as the questions to surface in the implementation PR's description.
- All commands assume a working directory at the repo root.

## Sequencing & dependencies

The dependency graph is bottom-up. Phase 6 must finish before anything in Phase 7 lands. Phase 7 must finish before Phase 8 (most of Phase 8). Phase 8's `feat-051` is the integration gate. Phase 9 depends on Phase 8. Phase 10 depends on Phase 8's `feat-051` and Phase 8's conformance suite (`feat-057`, which actually lives in Phase 10 but logically belongs to Phase 8's "make the harness real" beat). Phase 11 is deferred until Phase 8 is stable.

```
phase-6  feat-037  ──┐
phase-7  feat-038 ──┤
         feat-039 ──┤
         feat-040 ──┤
         feat-041 ──┤
         feat-042 ──┤
                    ├──>  feat-051 (Claude Code end-to-end)  ──┐
phase-8  feat-043 ──┤                                         │
         feat-044 ──┤                                         │
         feat-045 ──┤                                         ├──>  feat-054 (session layouts)
         feat-046 ──┤                                         │
         feat-047 ──┤                                         ├──>  feat-055 (kanban)
         feat-048 ──┤                                         │
         feat-049 ──┤                                         ├──>  feat-056 (A2A)
         feat-050 ──┘                                         │
                                                              │
phase-9  feat-052 ──────────────────────────────────────────┐ │
         feat-053 ──────────────────────────────────────────┤ │
         feat-055 ──────────────────────────────────────────┤ │
         feat-056 ──────────────────────────────────────────┘ │
                                                              │
phase-10 feat-057 (conformance) ──> feat-058 (Codex)          │
                              ──> feat-059 (OpenCode)        │
                                                              │
phase-11 feat-060 (attended, deferred) <──────────────────────┘
```

## Cross-cutting concerns

### Migration discipline

Every phase that adds schema columns or types MUST:

1. Add a new SQL file under `resources/migrations/` with the next numeric prefix (current latest: 009 from feat-029). Use `ALTER TABLE ... ADD COLUMN` for additive changes; never drop or rename existing columns. New columns that should be non-null in the running app must have a SQL `DEFAULT` value.
2. Update the `sessions` / `providers` / `columns` CREATE TABLE block in the bootstrap migration to include the new columns, so a fresh `init.sh` (clean DB) produces a schema that matches an upgraded one.
3. Update the matching `*Store` struct, the SELECT/INSERT/UPDATE SQL, and the JSON serialization. Run `cargo test -p weave-server -- test_migrations_idempotent` to confirm both code paths converge.
4. Add an integration test that starts with the prior schema version, runs migrations, and asserts the new columns exist with the right defaults. The test should also assert that re-running the migrations is a no-op.

### Testing patterns

- **Backend Rust unit tests** live next to the code (`#[cfg(test)] mod tests` in the source file). They use the in-memory `Db::open_memory()` helper from `crates/weave-server/src/db.rs` — do NOT use SQLite files in unit tests.
- **Integration tests** that need a real DB use the `Db::open_test` helper (or the existing `init_test_db` pattern) and isolate state per test. Look at `feat-008` for the pattern.
- **Conformance tests for CLI adapters** live in `crates/weave-server/tests/cli_conformance.rs` (Phase 10) and use the fake CLI binary from `crates/weave-server/tests/fakes/fake_cli/` (Phase 8). They exercise the adapter against the fake harness and never call a real CLI.
- **Frontend tests** are Vitest + React Testing Library. Component tests live in `web/src/**/*.test.tsx`. E2E for new session-creation wizard is a Phase 9 concern but is NOT a hard gate for that phase; unit tests + lint + typecheck are.
- **No new third-party crates** without a DECISIONS.md entry. The existing dependency set is the default.

### Coding-style notes (re-anchor the conventions for this work)

- **Immutability**: every state change is a new struct, never a mutation. The exception is `TraceCollector` channels and DB rows (those are inherently mutating stores).
- **Error messages**: include *what* went wrong (with file/line), *why* the rule exists, *how* to fix it. The session-exit hook will fail-stop a PR that ships vague error strings.
- **Path containment**: any CLI invocation that touches the filesystem must go through the existing `PathValidator` in `crates/weave-server/src/tools/fs/path_validator.rs`. Do not reimplement canonicalization; reuse.
- **Workspace scoping**: every DB query for sessions / providers / codebases / boards includes `workspace_id`. A2A requests resolve to a workspace via the resolved session; the workspace is NOT optional.
- **Streaming**: every new code path that produces assistant output goes through the SSE `SseManager` and emits `message_persisted` before `done`. Do not write directly to `messages` from a new path.

### Phase-by-phase rules

- **Phase 6 and Phase 7 are "no user-visible behavior change."** Every verification command is a Rust test that asserts existing API responses still match their old shape. Document the absence of regressions in the PR.
- **Phase 8 IS the first user-visible behavior change** (a new session can run on Claude Code). The fake CLI is the conformance target. A real `claude` binary is a manual smoke check, not a CI gate.
- **Phase 9 must preserve the existing `feat-020` / `feat-021` / `feat-022` frontend tests** verbatim. The 4-step wizard wraps the old modal; the old modal can be deleted only when the new wizard covers 100% of its flows.
- **Phase 10's conformance suite is a hard gate for Phase 10 itself.** feat-058 and feat-059 cannot land until the suite exists and feat-051 passes it.
- **Phase 11 is deferred.** No PR for feat-060 should be opened until a real `wrapped` session has been used for at least two weeks in production-like flow. The strategy doc §3 step 4 makes this explicit.

---

## Phase 6 — Native Tool Loop

### feat-037: Native Anthropic tool-execution loop

**Strategic context:** The strategy doc commits to "the first implementation plan should therefore lead with the native tool-execution loop feature, then create feature-list entries around Claude Code wrapped mode" (strategy §3 step 0). The reason: every other multi-runtime feature depends on the journey and trace shapes being real for the tools Weave advertises. Until `AnthropicAgent` actually runs those tools and records them in `traces`, the journey sidebar is lying.

**Goal:** `AnthropicAgent::send_message` consumes the SSE stream; when the provider emits a `tool_use` content block, the agent collects the streamed `input_json_delta`s into a `serde_json::Value`, runs the named tool through `ToolRegistry` subject to the session's profile, appends a `tool_result` content block, and continues the model call. The loop terminates on `end_turn` / `max_tokens` / `cancelled` / loop-limit-exceeded.

**Data shape changes:**
- `Message::role` gains a new variant `Tool` (already present in the docs as `tool_result`); ensure `MessageStore::create` accepts the new role and that `messages.content` JSON for the new role is `{ "type": "tool_result", "tool_use_id": "...", "content": "...", "is_error": bool }` (matches Anthropic's API shape).
- `StreamEvent::Done { stop_reason }` gains the loop-limit case: introduce `StopReason::LoopLimit` (or reuse `StopReason::MaxTokens` with a metadata flag — see Open decisions). The new behavior is observably different: a final assistant text is emitted saying "Sorry, too many tool calls" before the `done`.
- The persisted `messages.metadata` column for an aborted turn gains `{"stop_reason": "cancelled", "partial": true}` (or `"error"`) when a turn ends in cancellation or error. This is also the message that feat-036's `message_persisted` event will read.

**Implementation outline:**

1. Add the loop type. The cleanest pattern is a `while let Some(event) = stream.next().await` driver that accumulates content blocks into a `Vec<ContentBlock>`, watches for `AnthropicEvent::MessageStop`, and on `stop_reason=tool_use` dispatches a single tool call, appends the `tool_result` block to the history, and re-issues the request.
2. Build the request for the next iteration. The Anthropic Messages API takes the full message history; we hold the original `MessageRequest.messages` plus the accumulator and pass them back. The accumulator is `Vec<ContentBlock>` for the assistant turn-in-progress, plus the `tool_result` blocks for any tools the assistant called. Add a small test fixture that asserts the second request's body has the right shape.
3. Resolve the tool name against `ToolRegistry`. On unknown tool: append a `tool_result` with `is_error=true` and a structured error message; do not abort the loop. The model can recover by trying a different name. This is the Anthropic API's recommended pattern.
4. Validate input against the tool's JSON schema. Reuse `ToolExecutor::input_schema` (already a `serde_json::Value`). On validation failure, append a `tool_result` with `is_error=true` containing a schema-conformance error message. Do not abort the loop.
5. Run the tool. Use `tokio::time::timeout(per_tool_timeout, tool.execute(input, ctx))`. On timeout, append a `tool_result` with `is_error=true` and a clear message. Record the duration in the trace event.
6. Persist the tool_call trace event AFTER the tool result is sent back. The trace event should include: `name`, `input` (sanitized — strip values whose key matches `secret|key|token|password`), `output` (truncated to 10KB matching feat-017's existing behavior), `duration_ms`, `is_error`, `loop_iteration`.
7. Implement the loop limit. After the Nth iteration, instead of re-issuing the request, emit a final `TextDelta` ("Sorry, too many tool calls in a single turn."), persist a `Message` with `stop_reason=LoopLimit`, and emit `Done { stop_reason: LoopLimit }`. The session header pill (feat-054) surfaces this.
8. Implement cancellation. Wire the cancel token into the `tokio::select!` around the in-flight tool future. On cancel, persist the accumulated text with `stop_reason=cancelled` (via `messages.metadata`), emit `Done { stop_reason: Cancelled }`, and exit. Do not flush a `tool_result` for the in-flight tool; the assistant turn is considered abandoned.
9. Ensure the loop preserves Anthropic API behavior for non-tool turns: if the model never emits a `tool_use`, the loop runs exactly one iteration and exits with `end_turn`. The existing tests (`test_anthropic_sse_parsing`, `test_anthropic_error_mapping`) must still pass.

**Acceptance criteria:**
- [ ] `test_native_tool_loop_basic` — a script that emits a single `text_delta` then `tool_use` then a final `text_delta` plus `done` results in: (a) one tool call in `traces`, (b) the final persisted assistant message has both text blocks, (c) the loop ran exactly 2 iterations, (d) the SSE stream emits `Done { stop_reason: EndTurn }`.
- [ ] `test_native_tool_loop_unknown_tool` — the model emits a `tool_use` for a name not in the registry; the loop persists a `tool_result` with `is_error=true`, the loop continues, and the next iteration succeeds.
- [ ] `test_native_tool_loop_validation_error` — input fails the tool's JSON schema; the loop persists a `tool_result` with `is_error=true` containing the schema error, the loop continues.
- [ ] `test_native_tool_loop_exec_error` — the tool's `execute` returns `ToolResult { success: false, error: ... }`; the loop records the tool as failed in the trace, the loop continues.
- [ ] `test_native_tool_loop_limit` — a script that emits 9 consecutive `tool_use` blocks with `done` events causes the loop to exit with `StopReason::LoopLimit` after the 8th iteration, with a final "too many tool calls" assistant text persisted.
- [ ] `test_native_tool_loop_cancellation` — the cancel token is cancelled mid-tool-execution; the tool future is dropped, a `Done { stop_reason: Cancelled }` is emitted, and the persisted assistant message has `metadata.stop_reason=cancelled`.
- [ ] `test_native_tool_loop_no_tool_passes_through` — a model that never emits a `tool_use` runs exactly one iteration; the existing `test_anthropic_sse_parsing` and `test_anthropic_error_mapping` tests still pass.
- [ ] `test_native_tool_loop_preserves_trace_sanitization` — a tool input containing a key matching `secret` has its value replaced with `"***"` in the persisted `traces.data_json` (re-use the existing sanitizer in `trace/collector.rs`).
- [ ] `./init.sh` is green; existing 605 Rust tests + 83 frontend tests pass; no frontend changes in this feature.

**Verification:**
```bash
cargo test -p weave-server -- test_native_tool_loop_basic
cargo test -p weave-server -- test_native_tool_loop_unknown_tool
cargo test -p weave-server -- test_native_tool_loop_validation_error
cargo test -p weave-server -- test_native_tool_loop_exec_error
cargo test -p weave-server -- test_native_tool_loop_limit
cargo test -p weave-server -- test_native_tool_loop_cancellation
cargo test -p weave-server -- test_native_tool_loop_no_tool_passes_through
cargo test -p weave-server -- test_native_tool_loop_preserves_trace_sanitization
cargo test -p weave-server -- test_anthropic_sse_parsing
cargo test -p weave-server -- test_anthropic_error_mapping
./init.sh
```

**Design decisions already made:**
- `StopReason::LoopLimit` is a new variant; reusing `MaxTokens` would lose the user-visible distinction.
- The loop driver is a separate `AgentLoop` type, not inlined into `AnthropicAgent`. This keeps `AnthropicAgent::send_message` testable in isolation and is the same separation used by every other agent in the binary.
- Per-tool timeout default is 30s; the same default as `shell_exec` from feat-014.

**Design decisions open:**
- The cancel-during-tool case persists the in-flight tool as `cancelled` in the trace (not `error`). Confirm in the PR.
- "Sorry, too many tool calls" is a hardcoded English string. Decide if i18n is in-scope (almost certainly not for v1) and if a const should be hoisted to `agent::messages`.

**Dependencies:** feat-005, feat-006, feat-009, feat-012, feat-013.

---

## Phase 7 — Multi-Runtime Foundation

### feat-038: Session table migration for runtime/mode/cli_resume_id

**Strategic context:** §5 of the strategy: "Sessions need persistent runtime/mode and CLI resume metadata. Existing sessions backfill to Anthropic API native mode. CLI-specific state, including the CLI's native session id, lives in generic runtime metadata rather than one schema column per CLI." This feature is the SQL expression of that commitment.

**Goal:** Three new columns on `sessions`: `runtime_kind`, `mode`, and `runtime_metadata_json`. Backfill existing rows. The persistence layer round-trips them. No user-visible change.

**Data shape changes:**
- New migration `resources/migrations/010_session_runtime.sql`:
  ```sql
  ALTER TABLE sessions ADD COLUMN runtime_kind TEXT NOT NULL DEFAULT 'anthropic-api';
  ALTER TABLE sessions ADD COLUMN mode TEXT NOT NULL DEFAULT 'native';
  ALTER TABLE sessions ADD COLUMN runtime_metadata_json TEXT;
  -- Backfill is implicit via DEFAULT for runtime_kind and mode.
  ```
- Update the `sessions` CREATE TABLE block in the bootstrap migration so fresh DBs match upgraded ones.
- New `RuntimeKind` enum and `SessionMode` enum in the `agent` (or new `runtime`) module. `RuntimeKind` values: `AnthropicApi`, `OpenAiApi`, `OpenAiCompatible`, `ClaudeCode`, `Codex`, `Opencode`. `SessionMode` values: `Native`, `Wrapped`, `Attended`. Both derive `Serialize`, `Deserialize`, `Clone`, `Copy`, `Debug`, `PartialEq`, `Eq`.
- `Session` struct gains the three fields with appropriate `From<SqliteRow>` mapping. `runtime_metadata_json` is `Option<serde_json::Value>` from the outside; serialized as TEXT in the DB.
- `SessionStore::create` accepts a `CreateSessionRequest` that includes the three fields (defaulting to the v1 values when omitted, so existing call sites are unchanged). `get` / `list` return them. `update` allows updating `mode` and `runtime_metadata_json` (but NOT `runtime_kind` after creation — that would invalidate the in-flight agent; this is a hard rule).
- `parent_session_id` resume: when `parent_session_id` is set, the new session row inherits the parent's `runtime_kind` and `mode` by default. An override on the new session's `CreateSessionRequest` is allowed and is used instead.

**Implementation outline:**

1. Add the migration. Run the existing `test_migrations_idempotent` test against a fresh DB; the new columns must be present.
2. Add the `RuntimeKind` and `SessionMode` enums. Add a small helper to convert a string from the DB to the enum, with a clear error on unknown values (no silent fallback — a typo in a row should fail loud at read time).
3. Add the fields to `Session` and the `FromRow` impl. Update all SQL SELECT statements in `SessionStore` to include the new columns (or use `SELECT *` and rely on column order; the latter is fragile and the project already prefers explicit column lists).
4. Update `SessionStore::create` to accept the new fields. The `CreateSessionRequest` shape changes; the public API endpoint updates in a later feature (feat-040).
5. Wire `runtime_kind` and `mode` into the SSE `done` event payload. The frontend reads it in feat-054.
6. Add a `parent_session_id` resume test that asserts inheritance and override behavior.

**Acceptance criteria:**
- [ ] `test_session_runtime_kind_migration` — fresh DB has the new columns; upgraded DB has them; re-running the migration is a no-op.
- [ ] `test_session_runtime_metadata_roundtrip` — create a session with `runtime_metadata_json = {"cli_resume_id": "abc"}`, fetch it, parse the JSON, assert the resume id survived.
- [ ] `test_session_runtime_default_backfill` — upgrade path: open a DB with 9 existing rows, run the migration, all rows have `runtime_kind='anthropic-api'`, `mode='native'`, `runtime_metadata_json=NULL`.
- [ ] `test_session_runtime_kinds_in_db` — round-trip each variant of `RuntimeKind` and `SessionMode` through the store. Assert the strings match the strategy doc's commitment (kebab/snake case, not the Rust enum names).
- [ ] `test_session_parent_runtime_inherited` — create parent (kind=claude-code, mode=wrapped), create child via `parent_session_id` with no override, assert child has the parent's runtime/mode.
- [ ] `test_session_parent_runtime_override` — create parent (kind=claude-code), create child with explicit `runtime_kind=anthropic-api`, assert child has the override.
- [ ] `test_session_cannot_change_runtime_kind` — `SessionStore::update` rejects any update that touches `runtime_kind` with a clear error.
- [ ] `test_session_done_event_includes_runtime_mode` — the `SseWireEvent::Done` payload includes `runtime_kind` and `mode` fields; existing tests that don't check the new fields still pass.
- [ ] All existing session tests (`test_session_lifecycle`, `test_session_state_transitions`, `test_session_resume`, `test_session_resume_chain`) pass unchanged.

**Verification:**
```bash
cargo test -p weave-server -- test_session_runtime_kind_migration
cargo test -p weave-server -- test_session_runtime_metadata_roundtrip
cargo test -p weave-server -- test_session_runtime_default_backfill
cargo test -p weave-server -- test_session_runtime_kinds_in_db
cargo test -p weave-server -- test_session_parent_runtime_inherited
cargo test -p weave-server -- test_session_parent_runtime_override
cargo test -p weave-server -- test_session_cannot_change_runtime_kind
cargo test -p weave-server -- test_session_done_event_includes_runtime_mode
cargo test -p weave-server -- test_session_lifecycle
cargo test -p weave-server -- test_session_state_transitions
cargo test -p weave-server -- test_session_resume
./init.sh
```

**Design decisions already made:**
- `cli_resume_id` lives INSIDE `runtime_metadata_json`, NOT as its own column. This is the strategy's commitment to "generic runtime metadata rather than one schema column per CLI."
- `runtime_kind` is NOT changeable after creation. Changing runtime would invalidate the in-flight agent and any cached resume state. The right way to "switch runtime" is to create a new session.

**Design decisions open:**
- The wire format for `runtime_kind` in the API response. The existing convention is snake_case strings (`anthropic-api`, `claude-code`). Confirm in the PR.
- Should the API reject creating a session whose `runtime_kind` is `Attended`-capable today, or is that a feat-040 concern? Answer: feat-040.

**Dependencies:** feat-008.

---

### feat-039: Provider table config discriminated union

**Strategic context:** §5 of the strategy: "The internal Provider config becomes discriminated HTTP vs CLI config; the UI presents each registered row as a Runtime Tool." This feature widens the Provider table. The UI is updated separately in feat-052.

**Goal:** The `providers` table gains a `kind` discriminator and a CLI-specific config shape. `POST /api/providers` validates by kind. Existing HTTP rows keep working. The provider registry constructs a matching `CodingAgent` impl per kind (the CLI impl is a stub until feat-051 lands; for feat-039, the CLI kind is registered but `add_provider` returns an explicit "CLI runtime not yet enabled" error).

**Data shape changes:**
- New migration `resources/migrations/011_provider_kind.sql`:
  ```sql
  ALTER TABLE providers ADD COLUMN kind TEXT NOT NULL DEFAULT 'http';
  ALTER TABLE providers ADD COLUMN binary_path TEXT;
  ALTER TABLE providers ADD COLUMN args_json TEXT;
  ALTER TABLE providers ADD COLUMN env_json TEXT;
  ALTER TABLE providers ADD COLUMN permission_mode TEXT;
  -- Existing rows backfill to kind='http' implicitly.
  ```
- Update the `providers` CREATE TABLE block in the bootstrap migration.
- New `ProviderKind` enum: `Http`, `Cli`. Both rows keep `name` and `default_model`; HTTP rows use `base_url` / `api_key`; CLI rows use `binary_path` / `args_json` (Vec<String>) / `env_json` (BTreeMap<String, String>) / `permission_mode` (String, with the strategy's list of preset values).
- New validation: `POST /api/providers` rejects (a) `kind=http` with missing `base_url` or `api_key`; (b) `kind=cli` with missing `binary_path`; (c) `kind=cli` with `binary_path` not resolving to an executable on the current filesystem. The third check uses `tokio::fs::metadata` and a `+x` mode check on Unix.
- `GET /api/providers` and `GET /api/providers/:id` include `kind` and the kind-specific config. `api_key` is stripped from responses as today (extended to HTTP rows). CLI rows never had an `api_key`; the field is `None`.

**Implementation outline:**

1. Add the migration. Update the bootstrap migration's CREATE TABLE.
2. Add the `ProviderKind` enum, the `Provider` struct fields, and the `FromRow` mapping. Update all SQL in `ProviderStore` to include the new columns.
3. Add validation in the API handler: each `kind` requires its specific fields; missing fields return `AppError::Validation` with a `code: "missing_field"` and a message listing the missing field name.
4. Add the binary-executable check for `kind=cli`. The check is best-effort: it catches the obvious "did you type the right path" case. We do NOT block on it (e.g., a binary that doesn't exist yet but will be installed is a soft warning, not a hard error — emit a WARN log and accept the row).
5. Update `ProviderRegistry::add_provider`. For `kind=cli`, return an explicit `AppError::NotImplemented` with a clear message: "CLI runtime providers are added in feat-051; this row was persisted but no agent is registered yet." This lets users pre-register a CLI Provider row from the Settings UI today, even though it can't run a session.
6. Add an "advisory" / "draft" concept to the API. A CLI provider row persisted in feat-039 IS in the registry, but `add_provider` returns the NotImplemented error and the row is not load-bearing for any agent dispatch. When feat-051 lands, the registry will be re-initialized to find the same rows and dispatch them. Document this in the feature PR.
7. `GET /api/providers/:id/models` is HTTP-only in this feature; the model cache for CLI is feat-042.
8. Update `ProviderStore::remove_provider` to enforce the same "no active sessions" rule for CLI rows that exists for HTTP rows (feat-007).
9. Update the existing `test_provider_crud` and `test_provider_api_key_stripped` tests to assert the new fields are present and `kind='http'` for legacy rows. Add new tests for the CLI branch.

**Acceptance criteria:**
- [ ] `test_provider_kind_http_crud` — create / read / update / delete a `kind=http` row. Existing `test_provider_crud` continues to pass with the same wire shape.
- [ ] `test_provider_kind_cli_crud` — create / read / update / delete a `kind=cli` row. The row persists; the read returns the CLI-specific config; the delete succeeds if no active sessions reference it.
- [ ] `test_provider_kind_validation` — `kind=http` without `base_url` returns `code: "missing_field", field: "base_url"`. `kind=cli` without `binary_path` returns `code: "missing_field", field: "binary_path"`. `kind=cli` with non-existent `binary_path` logs a WARN and accepts the row (advisory, not blocking).
- [ ] `test_provider_api_key_stripped_across_kinds` — `GET /api/providers/:id` for a `kind=http` row strips `api_key`; for a `kind=cli` row, the response never includes an `api_key` field.
- [ ] `test_provider_migration_backfills_http` — upgrade path: open a DB with 7 existing providers, run migration, all rows have `kind='http'`, `binary_path=NULL`, etc.
- [ ] `test_provider_cli_row_not_yet_dispatchable` — `POST /api/providers` with `kind=cli` succeeds; `GET /api/providers` lists the row; `ProviderRegistry::get_agent` returns `Err(NotImplemented)`. Documented in the test.
- [ ] `test_provider_remove_referenced` — `DELETE /api/providers/:id` for a CLI row with an active session returns 409 (the existing rule from feat-007 applies to both kinds).
- [ ] All existing provider tests pass unchanged.

**Verification:**
```bash
cargo test -p weave-server -- test_provider_kind_http_crud
cargo test -p weave-server -- test_provider_kind_cli_crud
cargo test -p weave-server -- test_provider_kind_validation
cargo test -p weave-server -- test_provider_api_key_stripped_across_kinds
cargo test -p weave-server -- test_provider_migration_backfills_http
cargo test -p weave-server -- test_provider_cli_row_not_yet_dispatchable
cargo test -p weave-server -- test_provider_remove_referenced
cargo test -p weave-server -- test_provider_crud
cargo test -p weave-server -- test_provider_api_key_stripped
./init.sh
```

**Design decisions already made:**
- CLI providers can be pre-registered before they are dispatchable. This unblocks the Settings UI from feat-052 (which lets a user add a Claude Code row today, even if the adapter lands in feat-051).
- `binary_path` non-existence is a WARN, not an error. Production users may have a binary in `$HOME/.local/bin` that is not on the server's PATH; the user knows what they typed.
- The `Provider` struct's `args_json` and `env_json` are JSON-encoded; the API handler decodes them and the `add_provider` path takes the decoded `Vec<String>` / `BTreeMap<String, String>`.

**Design decisions open:**
- The list of accepted `permission_mode` values for the CLI kind. The strategy mentions `accept-edits` | `default` | `plan` | `bypass-permissions` but those are Claude-Code-specific. For feat-039, accept any non-empty string and let the per-adapter mapper (feat-046) validate. Each CLI kind's mapper will reject unknown values.
- Whether `default_model` is required for `kind=cli`. Decision: required (matches HTTP). A CLI row without a `default_model` would have to shell out to discover it on every list_models call, which the cache in feat-042 mitigates but does not eliminate.

**Dependencies:** feat-007.

---

### feat-040: Runtime Tool × mode compatibility validator

**Strategic context:** §4 of the strategy defines the runtime × mode matrix. This feature is the central place that matrix is enforced. Strategy §5 makes the explicit point: "A2A and kanban must stop silently choosing the first Provider once Runtime Tools are explicit. Multi-runtime implementation must add explicit Tool/provider selection for new A2A requests and kanban column bindings."

**Goal:** One function — `validate_runtime_mode_compat(runtime: RuntimeKind, mode: SessionMode) -> Result<(), AppError>` — that encodes the matrix. Session creation, kanban auto-spawn, and A2A all call it. Mismatches return a structured error.

**Data shape changes:**
- New `RuntimeKind` and `SessionMode` enums (defined in feat-038; this feature consumes them).
- New error variant on `AppError::Validation` (or a new `AppError::IncompatibleRuntimeMode` — see Open decisions) carrying `code: "runtime_mode_incompatible"`, `runtime: String`, `mode: String`, `supported_modes: Vec<String>` so the API response can list what the runtime actually supports.
- New public function in the `runtime` (or `agent`) module: `validate_runtime_mode_compat(runtime, mode)`.

**Implementation outline:**

1. Add the function. The matrix:
   - `AnthropicApi` / `OpenAiApi` / `OpenAiCompatible` → `native` only.
   - `ClaudeCode` / `Codex` / `Opencode` → `wrapped` only (today; `attended` is rejected until Phase 11).
2. Add the error variant. The wire shape:
   ```json
   {
     "error": {
       "code": "runtime_mode_incompatible",
       "message": "Runtime 'claude-code' supports mode 'wrapped', not 'native'",
       "details": { "runtime": "claude-code", "mode": "native", "supported_modes": ["wrapped"] }
     }
   }
   ```
3. Wire the call into session creation (handler in `api/sessions.rs` or wherever the create handler lives).
4. Wire the call into `try_automate_lane` (kanban auto-spawn).
5. Wire the call into A2A `POST /api/a2a/messages`.
6. Wire the call into `SessionStore::create` as a defense-in-depth check (in case a future caller forgets to call the validator). The store-level check duplicates the work but is cheap and catches regressions.
7. Add a test for each matrix cell.

**Acceptance criteria:**
- [ ] `test_runtime_mode_compat_anthropic_native_ok` — `validate_runtime_mode_compat(AnthropicApi, Native)` returns `Ok(())`.
- [ ] `test_runtime_mode_compat_anthropic_wrapped_rejected` — `validate_runtime_mode_compat(AnthropicApi, Wrapped)` returns `Err(IncompatibleRuntimeMode)` with the structured payload.
- [ ] `test_runtime_mode_compat_claude_code_wrapped_ok` — `validate_runtime_mode_compat(ClaudeCode, Wrapped)` returns `Ok(())`.
- [ ] `test_runtime_mode_compat_claude_code_native_rejected` — `validate_runtime_mode_compat(ClaudeCode, Native)` returns `Err(IncompatibleRuntimeMode)`.
- [ ] `test_runtime_mode_compat_attended_rejected_for_now` — `validate_runtime_mode_compat(ClaudeCode, Attended)` returns `Err(IncompatibleRuntimeMode)` with a `details.phase` field pointing at Phase 11 (the error is helpful, not just a generic "not allowed").
- [ ] `test_kanban_autospawn_rejects_incompatible_pair` — when a column's `runtime_kind` is `claude-code` and the workspace default mode is `native` (or vice versa), `try_automate_lane` returns the structured error and does NOT create a session.
- [ ] `test_a2a_rejects_incompatible_pair` — `POST /api/a2a/messages` with `runtime_kind=claude-code, mode=native` returns the structured error.
- [ ] `test_session_create_validates_compat` — `POST /api/workspaces/:wid/sessions` with an incompatible pair returns the structured error.
- [ ] `test_session_store_create_validates_compat` — direct call to `SessionStore::create` with an incompatible pair returns the same error.
- [ ] `test_runtime_mode_compat_matrix_exhaustive` — iterate every (runtime, mode) cell and assert the expected outcome; the test is the canonical spec of the matrix.

**Verification:**
```bash
cargo test -p weave-server -- test_runtime_mode_compat
cargo test -p weave-server -- test_kanban_autospawn_rejects_incompatible_pair
cargo test -p weave-server -- test_a2a_rejects_incompatible_pair
cargo test -p weave-server -- test_session_create_validates_compat
./init.sh
```

**Design decisions already made:**
- The matrix is enforced in three places (handler, service, store). The redundancy is intentional defense-in-depth; the cost is one extra comparison per session create.
- `attended` is rejected uniformly today, regardless of the CLI runtime kind. Phase 11 will relax this; until then, the error is the same.

**Design decisions open:**
- Whether `IncompatibleRuntimeMode` is a variant of `AppError::Validation` or a sibling. Sibling is cleaner (different status code semantics: validation is 400, this could be 422). Decide in the PR; both work.
- The exact wire shape of the error. Strategy doc does not specify; current `AppError::Validation` shapes follow `{ code, message, details? }` which is the convention.

**Dependencies:** feat-005, feat-038, feat-039.

---

### feat-041: CodingAgent trait extension for CLI turn context

**Strategic context:** §5 of the strategy: "CodingAgent trait shape must be revisited before the first CLI adapter lands. The existing stream contract is the right starting point, and the wrapped-CLI implementation should still be a `CliCodingAgent` alongside `AnthropicAgent`. But CLI-backed turns need per-session execution context — cwd/codebase, runtime metadata for CLI-native resume, effective permissions, and process lifecycle hooks — so the implementation plan must decide whether to extend `MessageRequest` or introduce a separate runtime-turn context." This feature commits to the separate context. The trait signature breaks; the implementation plan deliberately accepts the breakage because the trait is private to the binary.

**Goal:** `send_message` gains a `&TurnContext` parameter. `AnthropicAgent` (native mode) uses the subset of fields it cares about (cwd, cancel token) and ignores the rest. `CliCodingAgent` (added in feat-043 onward) uses the full struct. `SessionService::send_prompt` builds and threads the context.

**Data shape changes:**
- New `TurnContext` struct:
  ```rust
  pub struct TurnContext {
      pub session_id: SessionId,
      pub workspace_id: WorkspaceId,
      pub cwd: PathBuf,
      pub codebase_root: Option<PathBuf>,  // None for HTTP runtimes
      pub cli_resume_id: Option<String>,    // None for HTTP runtimes; populated by feat-047
      pub effective_permissions: PermissionSnapshot,  // Placeholder struct; real shape in feat-046
      pub cancellation_token: CancellationToken,
  }
  ```
- `PermissionSnapshot` is a placeholder in this feature: a struct with one field, `kind: RuntimeKind`, and a derived `Serialize`, `Deserialize`, `Clone`, `Debug`. The real fields land in feat-046. The placeholder exists so the trait signature is stable.
- `CodingAgent` trait change:
  ```rust
  pub trait CodingAgent: Send + Sync {
      fn provider_type(&self) -> &str;
      fn display_name(&self) -> &str;
      async fn list_models(&self) -> Result<Vec<ModelInfo>>;
      async fn send_message(
          &self,
          request: MessageRequest,
          turn: &TurnContext,
      ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>>>>>;
      async fn health_check(&self) -> Result<ProviderHealth>;
  }
  ```
- `MessageRequest` is unchanged. The strategy doc considered extending it; we chose not to. `MessageRequest` is the model-facing shape; `TurnContext` is the runtime-facing shape. Keeping them separate avoids leaking runtime concepts into the wire format the model sees.

**Implementation outline:**

1. Add `TurnContext` and the placeholder `PermissionSnapshot`. Place both in a new `agent::turn_context` module to keep the agent module from sprawling.
2. Update the `CodingAgent` trait. Audit the entire codebase for implementers (just `AnthropicAgent` today) and update its signature.
3. Update `AnthropicAgent::send_message` to accept the unused `&TurnContext`. The function does NOT need to read any field today, but the cancel token will be used in a future iteration of feat-037 if cancellation becomes per-tool rather than per-turn. The forward-compatible signature is worth the visual noise.
4. Update `SessionService::send_prompt` to construct a `TurnContext` from session state (load `cwd`, `codebase_root` from the matched codebase, `cli_resume_id` from `runtime_metadata_json` if present, the cancel token from the existing `ActiveSessions` map) and pass it to the agent.
5. Update every test that constructs an `AnthropicAgent` or calls `send_message` to pass a `&TurnContext`. Most of these are unit tests; a few are integration tests in `tests/`. The existing test helper `make_test_turn_context()` (or new) should be the canonical way to build a context in tests.
6. Update the conformance test scaffold (which is added in feat-057) to know the new signature.
7. Update `docs/provider-abstraction.md` to reflect the new signature. The existing example in the doc is `send_message(request)`; replace with `send_message(request, turn)`.

**Acceptance criteria:**
- [ ] `test_turn_context_construction` — build a `TurnContext` from a session row + codebase row; assert all fields populated correctly, including `codebase_root` resolving via the existing canonicalization helper.
- [ ] `test_turn_context_passes_cwd_and_codebase` — `AnthropicAgent::send_message` receives a `&TurnContext` whose `cwd` matches the session row's cwd; the agent does not panic and the existing test suite still passes.
- [ ] `test_anthropic_agent_signature_change_compiles` — explicit compile check: `cargo build -p weave-server` succeeds; the test exists as a regression guard.
- [ ] `test_session_service_passes_turn_context` — `SessionService::send_prompt` constructs a context with `cli_resume_id` from `runtime_metadata_json` when present, and `None` when absent.
- [ ] `test_turn_context_permission_snapshot_placeholder` — `PermissionSnapshot` serializes to JSON with the placeholder shape; the test is a regression guard for when feat-046 expands the struct.
- [ ] `test_turn_context_cancellation_token_cancelled` — the token in the constructed context is the same token registered in `ActiveSessions`; cancelling the registration cancels the token; the agent's loop (when feat-037 lands) reacts to the cancellation.
- [ ] `docs/provider-abstraction.md` is updated; the example signature in the doc matches the code.
- [ ] All existing agent tests pass with the new signature.

**Verification:**
```bash
cargo test -p weave-server -- test_turn_context
cargo test -p weave-server -- test_anthropic_agent_signature_change_compiles
cargo test -p weave-server -- test_session_service_passes_turn_context
cargo test -p weave-server -- test_anthropic_sse_parsing
cargo test -p weave-server -- test_anthropic_error_mapping
./init.sh
```

**Design decisions already made:**
- `MessageRequest` is NOT extended. Strategy doc's open question is resolved: separate context, not bigger request.
- `TurnContext` carries a `codebase_root` even for HTTP runtimes (it can be `None` for them). This keeps the struct shape uniform across runtimes; the agent decides whether to use it.
- `PermissionSnapshot` is a placeholder here. Adding the real shape in feat-046 lets feat-041 ship independently of feat-046.

**Design decisions open:**
- Should `TurnContext` carry the `TraceCollector` directly, or look it up from a registry by `session_id`? Decision in feat-041: carry it directly. Decision is reversible; flag for review in the PR.
- Should the `cancel` be a fresh `CancellationToken` per turn (cheap, allocated each call) or a session-scoped token reset on each prompt? Decision: session-scoped, reset on each prompt. This matches the existing `ActiveSessions` model from feat-034.

**Dependencies:** feat-005, feat-009, feat-038.

---

### feat-042: ProviderRegistry model cache

**Strategic context:** §6 of the strategy: "Models come from the Runtime Tool, not from Weave, and `list_models` needs its own cache. CLI model discovery shells out to the selected Runtime Tool and is slower than the existing provider health check. The implementation plan should add a longer-lived model cache keyed on Provider/Tool id, with explicit refresh and invalidation behavior. The existing 10s health-check TTL stays as-is." The key insight: the health-check TTL (10s) is right for "is this provider reachable?" but wrong for "what models does it have?" — the latter changes minutes-to-hours, not seconds.

**Goal:** A TTL-keyed model cache. HTTP `list_models` keeps its current behavior. CLI `list_models` shells out; results are cached. `add_provider` / `remove_provider` invalidate. `POST /api/providers/:id/refresh-models` is the explicit refresh path.

**Data shape changes:**
- New `ModelCache` type in the `agent` module. Key: `ProviderId`. Value: `Vec<ModelInfo>` plus a `cached_at: Instant`. Configurable TTL (default 5 minutes; env var `WEAVE_MODEL_CACHE_TTL_SECS`).
- The cache is in-process only. Restarting Weave drops the cache; this is intentional (a restart is itself a "refresh" event).
- `POST /api/providers/:id/refresh-models` returns the freshly-fetched list. For `kind=cli`, the call shells out; for `kind=http`, the call is a no-op refresh (HTTP `list_models` is fast enough to refresh on demand, but the cache still works).
- `add_provider` and `remove_provider` in the registry invalidate the relevant entry (and also invalidate the health cache from feat-033, which already happens).

**Implementation outline:**

1. Add the `ModelCache` type. A simple `HashMap<ProviderId, (Instant, Vec<ModelInfo>)>` behind a `tokio::sync::RwLock` is sufficient; we don't need LRU, just TTL. Document why in the code.
2. Wire it into `CodingAgent` impls. The trait doesn't change; the impl decides whether to consult the cache. `AnthropicAgent::list_models` continues to use the existing hardcoded Claude list. `CliCodingAgent::list_models` (added in feat-043) is the first user of the cache.
3. Add the `POST /api/providers/:id/refresh-models` endpoint. The handler is small: invalidate the cache entry, call the agent's `list_models`, return the result.
4. Reject `POST /api/providers/:id/refresh-models` for `kind=http` with a 405 — the endpoint is explicitly for CLI providers. (HTTP providers refresh on `list_models` anyway since the call is fast.) The HTTP path's `list_models` can also consult a small cache if we want, but it adds no value; leave it uncached.
5. Add a config option for the TTL. Wire the env var.
6. Add tests. The CLI branch is hard to test without a fake binary; use the fake CLI from feat-044 once it lands. For feat-042, the test targets are: cache hit, cache miss, invalidation on add/remove, refresh endpoint, TTL expiry.

**Acceptance criteria:**
- [ ] `test_model_cache_hit` — populate the cache; second `list_models` call within the TTL does NOT call the agent's `list_models` (use a counter on a wrapper agent to assert).
- [ ] `test_model_cache_miss_shells_out` — first call shells out (or in tests, calls the agent); result is cached; TTL is set to `cached_at + duration`.
- [ ] `test_model_cache_invalidation_on_add_remove` — add a provider, populate the cache, remove the provider, assert the cache entry is gone.
- [ ] `test_model_cache_refresh_endpoint` — `POST /api/providers/:id/refresh-models` returns the freshly-fetched list and the cache entry's `cached_at` is updated.
- [ ] `test_model_cache_refresh_rejected_for_http` — `POST /api/providers/:id/refresh-models` on a `kind=http` row returns 405.
- [ ] `test_model_cache_ttl_expiry` — populate the cache; advance the clock past the TTL; assert the next call shells out.
- [ ] `test_model_cache_health_cache_independent` — invalidating the model cache does NOT invalidate the 10s health cache; vice versa. The two caches have different lifetimes and concerns; test them separately.
- [ ] All existing health-check and provider tests pass unchanged.

**Verification:**
```bash
cargo test -p weave-server -- test_model_cache
cargo test -p weave-server -- test_health_check_detailed
cargo test -p weave-server -- test_provider_crud
./init.sh
```

**Design decisions already made:**
- The cache is in-process only; no DB persistence. This is intentional. A model list is cheap to recompute and a restart is a natural invalidation point.
- HTTP `list_models` does not consult the cache. The HTTP path is fast (in-memory hardcoded list); caching adds complexity for no benefit.
- The cache lives in the registry, not in each agent. Agents stay stateless; the registry is the coordinator.

**Design decisions open:**
- Whether to surface the cache age in `GET /api/providers/:id/models` (a `cached_at` field). Decision: yes, but only for CLI rows. Lets the UI show "models as of 3 minutes ago" if it cares. The HTTP response for HTTP rows is unchanged.
- Whether to add a background refresher that proactively refreshes the cache before TTL expiry. Decision: no. Adds complexity; CLI shell-outs are fast enough that 5-min staleness is fine.

**Dependencies:** feat-005, feat-007, feat-039.

---

## Phase 8 — Claude Code CLI Wrapped Mode

### feat-043: Per-turn CLI subprocess runner

**Strategic context:** §6 of the strategy: "Per-turn subprocess, not long-lived, for `wrapped` mode. Spawn the CLI per `send_message` with the resume flag. Long-lived preserves in-memory caches but couples Weave's session lifecycle to the CLI's process lifecycle. Start simple; revisit if a CLI's context-engine costs become visible. Cancellation and startup cleanup must account for child processes, not just database session rows." The runner is the spine of every `CliCodingAgent`.

**Goal:** A reusable `CliRunner` that spawns a registered CLI as a subprocess for one turn, captures stdout as a line stream and stderr to a bounded buffer, waits for exit or cancellation, and reaps the child. Per-turn, never long-lived. Tracks active children by session id for cancel and reap.

**Data shape changes:**
- New `CliRunner` type in the `agent` module. Constructed with a reference to the `ActiveChildProcesses` table (added in feat-049; in feat-043 it is a thin local map keyed by session id, replaced by the table in feat-049).
- New `CliInvocation` struct: `{ binary: PathBuf, args: Vec<String>, env: BTreeMap<String, String>, cwd: PathBuf, stdin_payload: Option<Vec<u8>> }`. The runner does NOT inspect `args` or `env`; it just passes them to `tokio::process::Command`.
- New `CliRunResult` enum: `Success { stdout: LineStream, stderr: Vec<u8>, exit_code: i32 }`, `Cancelled`, `SpawnError(io::Error)`, `ExitError { exit_code: i32, stderr: Vec<u8> }`. The `LineStream` is the type the parser in feat-045 consumes.
- New error variant: `AppError::CliProcess` with `code: "cli_process_failed"`, `code: "cli_spawn_failed"`, or `code: "cli_cancelled"`. The `cli_process_failed` variant carries the exit code and a sanitized stderr preview.

**Implementation outline:**

1. Build the `CliRunner::run(invocation, turn) -> Result<CliRunResult>` method. The signature accepts a `&TurnContext` (from feat-041) and uses its `cancellation_token` and `session_id`.
2. Spawn the process. `tokio::process::Command::new(binary).args(args).envs(env).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()`. Capture the pid. Register the child in the active-processes table keyed by `session_id`.
3. Spawn a `tokio::spawn` task that reads stdout line-by-line and pushes them onto a `tokio::sync::mpsc::Sender<String>`. The parser in feat-045 consumes from the receiver. Lines longer than 1MB are split (the Claude Code CLI is well-behaved; this is a defense).
4. Capture stderr in another task: `Vec<u8>` bounded to 256KB; if exceeded, truncate with a "stderr truncated" marker. After the process exits, the buffer is exposed in `CliRunResult` for error reporting.
5. Wire the cancel token via `tokio::select!`. On cancel: look up the child in the active-processes table, send SIGTERM via `libc::kill(pid as i32, SIGTERM)` (Unix-only; on Windows, use a different approach — but Weave is Linux-first), wait up to 5s for graceful exit, then SIGKILL.
6. On process exit (natural or killed), wait with `child.wait().await` to reap the zombie. Remove the entry from the active-processes table.
7. Map exit code to `CliRunResult`. 0 → `Success`. Non-zero → `ExitError { exit_code, stderr }`. SIGTERM-induced exit → `Cancelled`.
8. Re-use the existing shell output truncation policy (100KB on stdout lines; the parser handles lines internally).
9. Add a per-turn log: log the full argv (with env keys but not values, to avoid leaking secrets), the cwd, the session id, and the exit code. Useful for debugging; safe to log.

**Acceptance criteria:**
- [ ] `test_cli_runner_basic` — invoke `/bin/echo` with args `["hello", "world"]`; assert `Success` with the captured line.
- [ ] `test_cli_runner_cwd_env_args` — invoke a script that prints its cwd and the value of an env var passed via `env`; assert the runner's `cwd` and the script's stdout match.
- [ ] `test_cli_runner_cancel_sends_sigterm` — spawn `/bin/sleep 30`, cancel after 100ms, assert the process exits within 1s and `CliRunResult::Cancelled` is returned.
- [ ] `test_cli_runner_exit_nonzero_maps_to_error` — spawn `/bin/false`; assert `ExitError { exit_code: 1 }`.
- [ ] `test_cli_runner_stderr_capture` — spawn a script that writes to stderr; assert `stderr` is captured and bounded.
- [ ] `test_cli_runner_per_turn_process_table` — spawn two processes for two different session ids concurrently; both succeed; the active-processes table is empty after both exit.
- [ ] `test_cli_runner_reuse_after_exit` — spawn a process, let it exit, spawn another for the same session id; both succeed (no stale state).
- [ ] `test_cli_runner_registers_session_id` — the active-processes table has the right entry while the process is running; the entry is gone after exit.
- [ ] `test_cli_runner_log_redacts_env_values` — the log line for a turn includes the env keys but not their values. Assert by parsing the log output (use `tracing-subscriber`'s test layer).

**Verification:**
```bash
cargo test -p weave-server -- test_cli_runner
./init.sh
```

**Design decisions already made:**
- Stdin is `Option<Vec<u8>>`. Some CLIs read the prompt from stdin (Claude Code does, in `--input-format stream-json` mode); others take it as argv. The runner supports both.
- SIGTERM, then SIGKILL after 5s. Matches the existing graceful-shutdown pattern from feat-034.
- Per-turn means: no shared state across turns. The next turn's runner is a fresh spawn. This keeps the runner simple and matches the strategy's "start simple" call.

**Design decisions open:**
- The active-processes table's data structure. feat-043 uses a `tokio::sync::Mutex<HashMap<SessionId, (Pid, Child)>>`; feat-049 may refine it. Confirm the refactor is a non-event in feat-049.
- The behavior when a cancel arrives after the process has already exited but before the wait completes. The current plan: cancel becomes a no-op. Document in the test.

**Dependencies:** feat-009, feat-041.

---

### feat-044: Fake CLI test harness (conformance fixture)

**Strategic context:** §3 of the strategy: "Build the CLI subprocess harness, fake CLI test fixture, stream parser, permission mapper, runtime session-id capture, resume behavior, cancellation behavior, and journey translation around Claude Code." The fake CLI is the linchpin: every Phase 8 and Phase 10 test depends on a deterministic, scriptable binary that speaks the Claude Code (and later Codex, OpenCode) wire format. Real CLIs are out of CI; the fake is the conformance target.

**Goal:** A test-only `[[bin]]` target `fake_cli` that emulates a Claude-Code-style CLI. Reads prompts from stdin or argv (env-controlled), emits a deterministic event sequence on stdout, supports a handful of failure scenarios. Echoes back the resume id so tests can assert resume behavior. Lives entirely in test code; never shipped in production binaries.

**Data shape changes:**
- New crate target `crates/weave-server/tests/fakes/fake_cli/`. A `Cargo.toml` for the binary plus the source. Built by `cargo test -p weave-server` (cargo discovers it as a `[[bin]]` automatically when listed in `weave-server`'s test scope).
- The fake's behavior is controlled by env vars: `FAKE_CLI_SCRIPT` (one of `text-only`, `text+tool+done`, `permission-denied`, `crash`, `resume-unknown-session`, `echo-resume-id`), `FAKE_CLI_INPUT_MODE` (one of `stdin`, `argv`), `FAKE_CLI_DELAY_MS` (sleeps between events to simulate latency).
- Output is a line stream of JSON objects, one per line. Each line is a valid Claude Code event: `{"type": "session_id", "id": "..."}`, `{"type": "text_delta", "text": "..."}`, `{"type": "tool_use", "id": "...", "name": "...", "input": {...}}`, `{"type": "tool_result", "tool_use_id": "...", "content": "..."}`, `{"type": "thinking", "text": "..."}`, `{"type": "error", "message": "..."}`, `{"type": "done", "stop_reason": "end_turn"}`.
- The fake echoes the resume id it received (via `--resume` or stdin field) in its first emitted `session_id` event, so tests can assert the runner passed it.

**Implementation outline:**

1. Create the crate. Use `clap` (or simple `std::env::args`) for argv parsing. The binary is ~150 lines of Rust; do not over-engineer.
2. Implement the scripts. Each script is a `match env::var("FAKE_CLI_SCRIPT")` branch. The branches are small; share helpers for the event-emission format.
3. Implement the input mode. `stdin` mode reads the prompt from stdin (the runner writes the prompt via stdin in feat-051). `argv` mode reads the prompt from `--prompt <text>`.
4. Implement the resume id echo. The fake parses `--resume <id>` from argv (or a `resume` field in stdin JSON) and emits `{"type": "session_id", "id": "<id>"}` as its first event. This is the contract feat-047 depends on.
5. Add the test scenarios. Each scenario is a small Rust test that spawns the fake binary and asserts on its output (captured via the runner from feat-043).
6. Document the fake's wire format in `tests/fakes/fake_cli/README.md` so future adapter authors know what to emulate.

**Acceptance criteria:**
- [ ] `test_fake_cli_emits_text_delta` — `FAKE_CLI_SCRIPT=text-only`; the fake emits a `text_delta` then a `done`; the test captures both via the runner.
- [ ] `test_fake_cli_emits_tool_use_and_result` — `FAKE_CLI_SCRIPT=text+tool+done`; the fake emits a `text_delta`, a `tool_use` (with id, name, input), and a `done`; the test asserts the events are in order.
- [ ] `test_fake_cli_permission_denied_scenario` — the fake emits an `error` event with `code: "permission_denied"` and exits with code 2; the test asserts the runner maps this to `CliRunResult::ExitError { exit_code: 2, stderr_preview_contains("permission_denied") }`.
- [ ] `test_fake_cli_crash_scenario` — the fake exits with code 139 (SIGSEGV emulation via `std::process::exit(139)`); the test asserts `ExitError { exit_code: 139 }`.
- [ ] `test_fake_cli_resume_unknown_session` — the fake receives `--resume unknown-id` and emits an `error` event with `code: "resume_unknown_session"` and exits with code 3; the test asserts the runner surfaces this so feat-047 can fall back to replay.
- [ ] `test_fake_cli_echoes_resume_id` — the fake receives `--resume abc-123` and emits `{"type": "session_id", "id": "abc-123"}` as its first event; the test asserts the parser captures the id (and the runner forwards it into `TurnContext::cli_resume_id`).
- [ ] `test_fake_cli_input_mode_stdin` — `FAKE_CLI_INPUT_MODE=stdin`; the runner writes the prompt to the fake's stdin; the fake reads it and includes it in its first event's `prompt_received` field.
- [ ] `test_fake_cli_input_mode_argv` — same as above but with `argv` mode.

**Verification:**
```bash
cargo test -p weave-server -- test_fake_cli
cargo build -p weave-server --bin fake_cli  # explicit build check
./init.sh
```

**Design decisions already made:**
- The fake is a separate `[[bin]]`, not a library. The harness is a black box from the test's perspective; this enforces the contract.
- Event format matches Claude Code's actual `stream-json` shape. The fake is an emulator; real CLIs are expected to match. The conformance test in feat-057 will assert real CLIs (in a manual smoke test) match this shape.
- The fake's scripts are env-var controlled, not argv. Argv is reserved for the prompt, resume id, and Claude-Code-style flags. This keeps the fake's test surface small.

**Design decisions open:**
- Whether the fake should be its own crate (e.g., `crates/fake-cli`) or live inside `weave-server`'s `tests/fakes/`. Current decision: inside `weave-server` as a `[[bin]]` listed only in test scope. Revisit if the fake grows.
- Whether to support multiple "personalities" (Claude Code vs Codex vs OpenCode) in one binary or to fork it per adapter. Current decision: one binary per adapter, each with its own event grammar. Cleaner contracts.

**Dependencies:** (none — pure test infrastructure).

---

### feat-045: Claude Code `stream-json` parser

**Strategic context:** §6 of the strategy: "Normalize runtime output. CLI output maps into the existing `StreamEvent` contract." The parser is the boundary that makes "any CLI" look like "any HTTP provider" to the rest of the system.

**Goal:** A line-stream parser that consumes Claude Code's `stream-json` output and emits `StreamEvent`s. State machine that tracks in-flight content blocks. Captures the CLI's session id. Malformed lines are logged and skipped, never fatal.

**Data shape changes:**
- New `ClaudeCodeStreamParser` type in the `agent::adapters::claude_code` (or wherever the Claude Code adapter lives) module. Implements a `Stream<StreamEvent>` over the line stream from feat-043.
- The parser is a state machine. It tracks:
  - The CLI's session id (from the first `session_id` event). Captured into a `Sender<String>` so the runner (or the adapter) can write it into `TurnContext::cli_resume_id` for feat-047.
  - In-flight `content_block`s by their CLI-assigned ids. Reconciles them with the Weave `tool_use` id (which is the same id, but explicit).
  - The current `stop_reason` (from the `done` event).
- The parser's output is `StreamEvent`. It does NOT execute tools, parse file-change announcements, or build journey trace events — those are feat-048.

**Implementation outline:**

1. Define the parser's input contract: a `Stream<Item = String>` of newline-stripped lines. The runner from feat-043 produces this; the parser consumes it.
2. Define the parser's output contract: a `Stream<Item = Result<StreamEvent, ParseError>>`. `ParseError` is logged-and-skipped, not propagated; the stream continues.
3. Implement the state machine. The transitions are simple — line-by-line. The only stateful part is the in-flight content block tracking, which exists to handle the case where a `text_delta` arrives before the corresponding `content_block_start` (defensive; Claude Code doesn't do this, but the parser shouldn't assume).
4. Map events. `text_delta` → `TextDelta`. `tool_use` (start) → `ToolUseStart { id, name, input: serde_json::Value::Null }`; the `input` is filled as `input_json_delta` events arrive. `input_json_delta` → `ToolUseDelta { id, delta }`. `tool_result` → `ToolResult { id, result }` (serializing `content` to a string). `thinking` → `Thinking`. `error` → `Error { message }`. `done` → `Done { stop_reason }`. `session_id` → captured into the `Sender<String>`.
5. Handle malformed input. If a line is not valid JSON, log at WARN with the line preview (truncated) and skip. If a line has an unknown `type`, log at WARN and skip. If a `tool_use` references an id that wasn't preceded by a `content_block_start` for a `tool_use` block, log at WARN and skip the delta but keep the parser running.
6. Test the parser with the fake CLI from feat-044. The conformance test (feat-057) eventually becomes the canonical spec; feat-045's tests are the per-event sanity checks.

**Acceptance criteria:**
- [ ] `test_claude_code_parser_text_delta` — input: a single `text_delta` line. Output: `Ok(StreamEvent::TextDelta { text: "..." })`.
- [ ] `test_claude_code_parser_tool_use_start_delta` — input: `content_block_start` (type=tool_use) + a few `input_json_delta` lines + `content_block_stop`. Output: `ToolUseStart` + N × `ToolUseDelta`. (Note: this assumes Claude Code's wire format uses the Anthropic-style content-block framing; the parser must match the real format, which may differ. Confirm during implementation.)
- [ ] `test_claude_code_parser_tool_result` — input: a `tool_result` line. Output: `ToolResult` with the right id and serialized content.
- [ ] `test_claude_code_parser_thinking` — input: a `thinking` line. Output: `Thinking`.
- [ ] `test_claude_code_parser_session_id_capture` — input: a `session_id` line. Output: nothing on the event stream, but the `Sender<String>` receives the id. Test by reading from the receiver.
- [ ] `test_claude_code_parser_done_stop_reason` — input: a `done` line with `stop_reason: "end_turn"`. Output: `Done { stop_reason: EndTurn }`. Repeat for `max_tokens` and `tool_use`.
- [ ] `test_claude_code_parser_malformed_line_skipped` — input: a line of garbage. Output: no event (the line is logged). Subsequent valid lines still produce events.
- [ ] `test_claude_code_parser_unknown_event_type` — input: a valid JSON line with `type: "future_event_we_dont_know"`. Output: no event; WARN logged.
- [ ] `test_claude_code_parser_uses_fake_cli` — full end-to-end: feed the fake CLI's output through the parser; assert the parsed event sequence matches the script's expected events. (This is a precursor to feat-057's conformance suite.)

**Verification:**
```bash
cargo test -p weave-server -- test_claude_code_parser
./init.sh
```

**Design decisions already made:**
- The parser is stateless from the agent's perspective — it just emits events. The runner is responsible for sending the session id to where it needs to go.
- Malformed lines are logged, not surfaced. The journey sidebar will show "the CLI emitted N malformed lines" as an `error` trace event in feat-048, but the parser itself never aborts.
- The parser's tests do NOT depend on feat-043 (the runner) or feat-047 (the resume logic). The parser is a pure function from lines to events; the integration with the runner is feat-051.

**Design decisions open:**
- The exact mapping from Claude Code's actual wire format to `StreamEvent`. The strategy doc describes Claude Code's behavior at a high level; the actual format is in Claude Code's docs. The implementer must read the Claude Code docs during feat-045 and update this task description with any deviations from the assumed shape.
- Whether `input_json_delta` accumulates into a `serde_json::Value` inside the parser (so `ToolUseDelta` carries a `serde_json::Value` delta) or stays as a raw string. Decision: stay as a raw string (`String`) and let the consumer parse on receipt. This is more flexible; the journey translator in feat-048 may want the raw string for sanitization.

**Dependencies:** feat-005.

---

### feat-046: `PermissionMapper` trait + Claude Code implementation

**Strategic context:** §6 of the strategy: "Permission mode is per Runtime Tool, and per provider/runtime kind. Different CLIs handle permissions differently, and a `ToolProfile` cannot map to one universal flag set. The implementation plan must define a `PermissionMapper` contract and concrete effective-permission snapshots, starting with Claude Code and stubbing later CLIs only as far as the CLI adapter contract requires." This feature defines the contract and ships the first concrete impl.

**Goal:** A `PermissionMapper` trait, plus the Claude Code impl that maps a Weave `ToolProfile` to a Claude Code `PermissionSnapshot`. The snapshot is passed through `TurnContext` (feat-041) and consumed by the runner (feat-043) to build the CLI's argv on each turn.

**Data shape changes:**
- Replace the placeholder `PermissionSnapshot` from feat-041 with the real struct:
  ```rust
  pub struct PermissionSnapshot {
      pub runtime_kind: RuntimeKind,
      pub profile: ToolProfile,
      pub cli_flags: Vec<String>,         // e.g., ["--permission-mode", "accept-edits"]
      pub allowed_tools: Vec<String>,     // mapped from profile
      pub env_overrides: BTreeMap<String, String>,  // optional env vars
  }
  ```
- New `PermissionMapper` trait:
  ```rust
  #[async_trait]
  pub trait PermissionMapper: Send + Sync {
      fn runtime_kind(&self) -> RuntimeKind;
      fn effective_permissions(&self, profile: ToolProfile) -> PermissionSnapshot;
  }
  ```
- Concrete `ClaudeCodePermissionMapper` impl. Mapping table:
  - `full` → `--permission-mode bypass-permissions`, no tool allowlist restriction.
  - `implementation` → `--permission-mode accept-edits`, allowed tools = filesystem + shell + git + task context (matches Weave's `implementation` profile).
  - `review` → `--permission-mode plan`, allowed tools = filesystem read-only + git + task context + artifacts.
  - `planning` → `--permission-mode plan`, allowed tools = task context + kanban + notes.
  - `reporting` → `--permission-mode default`, allowed tools = task context read-only + notes + artifacts.
- Snapshot serialization: `serde::Serialize` + `serde::Deserialize`. The runner may log it at DEBUG on each turn.

**Implementation outline:**

1. Update `PermissionSnapshot` (was a placeholder) to the real struct. Add `Serialize` / `Deserialize` and a custom `Debug` impl that redacts `env_overrides` values (the values may contain secrets; the keys are safe to log).
2. Add the `PermissionMapper` trait in the `agent::permissions` module.
3. Add the `ClaudeCodePermissionMapper` impl. The mapping is a static `match`; the only state is the runtime kind.
4. Register the mapper in the `PermissionMapperRegistry` (a `HashMap<RuntimeKind, Arc<dyn PermissionMapper>>`). The registry is built at startup; for feat-046, only `ClaudeCode` is registered. Later phases (Phase 10) add Codex and OpenCode.
5. In `SessionService::send_prompt`, look up the mapper for the session's `RuntimeKind`, call `effective_permissions(profile)`, and put the result into `TurnContext::effective_permissions`. If the kind is HTTP, the snapshot is a default `PermissionSnapshot { cli_flags: vec![], allowed_tools: vec![], env_overrides: Default::default() }` — the runner doesn't read it for HTTP runtimes.
6. Add tests for each profile mapping. Each test asserts the snapshot's `cli_flags` and `allowed_tools`.

**Acceptance criteria:**
- [ ] `test_permission_mapper_trait_compiles` — the trait compiles; the test is a regression guard.
- [ ] `test_permission_mapper_claude_code_full` — `effective_permissions(full)` returns a snapshot with `cli_flags` containing `--permission-mode bypass-permissions` and `allowed_tools` empty (or all-tools).
- [ ] `test_permission_mapper_claude_code_implementation` — `effective_permissions(implementation)` returns `--permission-mode accept-edits` and `allowed_tools` matching the Weave implementation profile's tool set.
- [ ] `test_permission_mapper_claude_code_review` — `--permission-mode plan`, allowed tools are read-only.
- [ ] `test_permission_mapper_claude_code_planning` — `--permission-mode plan`, allowed tools are task/kanban/notes.
- [ ] `test_permission_mapper_claude_code_reporting` — `--permission-mode default`, allowed tools are read-only task/notes/artifacts.
- [ ] `test_permission_snapshot_serializes_to_json` — round-trip through serde; assert the JSON has the expected fields.
- [ ] `test_permission_snapshot_debug_redacts_env_values` — `format!("{:?}", snapshot)` does NOT include env values (only keys). Assert by parsing the debug output.
- [ ] `test_permission_mapper_registry_lookup` — looking up an unregistered kind (e.g., `Codex` before Phase 10) returns the HTTP default snapshot; not an error.

**Verification:**
```bash
cargo test -p weave-server -- test_permission_mapper
cargo test -p weave-server -- test_permission_snapshot
./init.sh
```

**Design decisions already made:**
- The mapping is a static table, not configurable per session. Strategy §6 says "Permission mode is per Runtime Tool, and per provider/runtime kind" — not per session. Per-session permission overrides are a future feature, not in v1.
- `env_overrides` exists in the struct from day 1 even though no current mapper uses it. Codex and OpenCode will likely need it; adding it later is a struct-change regression.

**Design decisions open:**
- Whether the `allowed_tools` list is enforced by Weave OR by the CLI. Strategy §6 says "the CLI runs its own tools; Weave translates the CLI's tool calls into the same UI and trace shapes, but does NOT re-execute them." So enforcement is the CLI's job; Weave's `allowed_tools` is informational, surfaced in the snapshot for the journey sidebar. Confirm in the PR.
- The exact value of `bypass-permissions` for the `full` profile. Some users will not want this — it skips all Claude Code confirmations. The strategy doc does not flag this as a concern; the `full` profile in Weave already means "no restrictions," and the `full` profile is opt-in (specialists choose their profile).

**Dependencies:** feat-005, feat-012, feat-040, feat-041.

---

### feat-047: CLI resume metadata persistence + replay fallback

**Strategic context:** §6 of the strategy: "The CLI's own session id is the durable key for resume, not the Weave session id. Store the CLI id in runtime metadata and pass it to the CLI on the next turn when present. If the CLI rejects it or the user switched Runtime Tools, fall back to message-history replay. Surface the chosen path in the session header so the user knows which mode they're in." This is the mechanism.

**Goal:** After a successful CLI turn, the CLI's native session id is captured (by the parser in feat-045) and stored in `sessions.runtime_metadata_json['cli_resume_id']`. On the next `send_message`, the runner passes it to the CLI. If the CLI rejects the resume id, the runner clears it and falls back to message-history replay (rebuild context from `messages` and re-invoke without the resume flag). The `done` SSE event includes a `resume_state` so the UI can show it.

**Data shape changes:**
- `sessions.runtime_metadata_json` is a `serde_json::Value`. The convention is: well-known keys at the top level (`cli_resume_id`, `last_replay_at`, `replay_count`). Per-adapter keys are namespaced by adapter (e.g., `claude_code_session_id` as an alias for the strategy's "CLI's own session id"). Use the namespaced form so multiple CLIs could coexist in metadata.
- The `done` SSE event payload gains a `resume_state: "none" | "native" | "replayed"` field. `none` = first turn, no resume attempted. `native` = CLI accepted the resume id. `replayed` = CLI rejected the resume id, the runner fell back to message-history replay.
- New error variant: `AppError::ResumeFallback` carrying the reason the resume was rejected (parsed from the CLI's stderr or stdout). This is a non-fatal error — the session continues with replay.

**Implementation outline:**

1. In the runner, after the CLI process exits successfully, read the `session_id` captured by the parser. Write it into `runtime_metadata_json['cli_resume_id'] = "<id>"`. Use the existing `SessionStore::update` mechanism (which already supports updating `runtime_metadata_json` per feat-038).
2. On the next turn, before spawning, read `runtime_metadata_json['cli_resume_id']`. If present, add `--resume <id>` to the argv (or the stdin payload, depending on the CLI's contract). The CLI knows the resume flag; the adapter knows the flag shape.
3. If the CLI exits non-zero and the stderr / stdout contains `resume_unknown_session` (or any structured error indicating the resume id is stale), do the following:
   - Clear `runtime_metadata_json['cli_resume_id']` (set to `null`).
   - Read all messages for the session from `MessageStore`.
   - Construct a `MessageRequest` whose `messages` are the full history (user + assistant + tool_result), as if the resume id was never used.
   - Re-spawn the CLI with the rebuilt `MessageRequest` and NO resume flag.
   - On success, set `resume_state = "replayed"` on the `done` event.
4. If the user changes `runtime_kind` on a new session, the prior session's `cli_resume_id` is NOT inherited. The `parent_session_id` resume in feat-018 does not copy `runtime_metadata_json`; the new session starts fresh.
5. The `done` event payload is updated to include `resume_state`. The frontend reads this in feat-054.
6. Tests: write scripts against the fake CLI that exercise (a) first turn (resume_state=none), (b) successful resume (resume_state=native), (c) rejected resume with fallback (resume_state=replayed).

**Acceptance criteria:**
- [ ] `test_cli_resume_id_persisted_after_turn` — run a turn with the fake CLI that emits a `session_id`; assert `runtime_metadata_json['cli_resume_id']` matches the emitted id after the turn.
- [ ] `test_cli_resume_id_passed_on_next_turn` — first turn runs with no resume flag; second turn runs with `--resume <id>` matching the persisted id. Assert via the fake's resume-echo behavior.
- [ ] `test_cli_resume_fallback_replay_on_unknown` — first turn persists an id; second turn's fake emits `resume_unknown_session` and exits 3; assert the runner clears the id, re-spawns without the resume flag, the session succeeds, and `resume_state="replayed"` is in the `done` event.
- [ ] `test_cli_resume_cleared_after_replay` — after a replay, `runtime_metadata_json['cli_resume_id']` is `null` (or absent).
- [ ] `test_cli_resume_not_inherited_across_runtime_switch` — create a parent session with `runtime_kind=claude-code` and a persisted `cli_resume_id`; create a child session with `parent_session_id=<parent>.id` and `runtime_kind=anthropic-api`; assert the child has no `cli_resume_id`.
- [ ] `test_cli_resume_state_in_sse_done` — the SSE `done` event for each scenario above includes the right `resume_state` value.
- [ ] `test_cli_resume_id_never_logged` — the resume id is NOT logged at INFO or above; the runner's debug log may include it but the user-facing SSE stream does not. Assert via log capture.

**Verification:**
```bash
cargo test -p weave-server -- test_cli_resume
./init.sh
```

**Design decisions already made:**
- The resume flag is adapter-specific. The runner does not know it; the adapter (feat-051 for Claude Code) is responsible for adding it to the argv.
- The fallback is "rebuild from `messages` and re-spawn without the resume flag." We do NOT support a "no-history, just system prompt" path; that would lose the user's prior context.
- `resume_state` is in the `done` event, not as a separate SSE event. The session page reads it from the `done` event in feat-054.

**Design decisions open:**
- Whether the replay path is also used when the CLI is replaced (e.g., the user uninstalls Claude Code mid-session). The current plan: yes — if no CLI matches the persisted `binary_path`, the runner falls back to replay with the new binary (if a `runtime_kind` is set) or fails. This is a corner case; document it.
- Whether to also persist a `replay_count` in `runtime_metadata_json`. Useful for debugging ("this session has been replayed 4 times"); not user-visible. Add the field, default 0.

**Dependencies:** feat-005, feat-008, feat-038, feat-041, feat-043, feat-045.

---

### feat-048: `JourneyTranslator` for CLI streams

**Strategic context:** §6 of the strategy: "The journey sidebar is the unifying artifact, for both fixed native mode and wrapped mode. Native mode becomes honest once §3 step 0 records tool execution and file changes through Weave's `ToolExecutor` path. Wrapped mode translates each CLI's thinking/tool/file-change stream into the same trace shapes, without re-executing CLI tools. That is the single thing that makes 'see what all of them did' real." This feature is the wrapped-mode half of that commitment. The native-mode half is feat-037.

**Goal:** A `JourneyTranslator` that maps the parsed CLI stream events into Weave trace events (`tool_call`, `file_change`, `decision`, `error`) and into the corresponding `StreamEvent`s for the SSE stream. The CLI is the source of truth for tool results — Weave does NOT re-execute the tool. The journey sidebar (feat-022) is unchanged because the trace shape is the same.

**Data shape changes:**
- New `JourneyTranslator` type. State: in-flight tool calls (id → start time), pending file changes, pending decisions.
- New `FileChangeAnnouncement` event in the parser's input. Claude Code emits file changes via a CLI-specific event; the parser passes them through, the translator consumes them. (If Claude Code's wire format does NOT emit file changes, this becomes a Phase 10 concern: the implementer must read Claude Code's docs and update the translator's input contract.)
- New `trace::Event::FileChange` variant is NOT added; the existing `file_changes` table is used (per `docs/data-model.md`).
- New trace metadata fields: `tool_call.cli_managed: bool` (true for CLI-originated calls, false for Weave-executed calls) so the journey sidebar can render the distinction. `decision.source: "claude_code" | "anthropic_api" | ...` to attribute decisions to the runtime.

**Implementation outline:**

1. Define the translator's input contract: a `Stream<StreamEvent>` from feat-045. The translator wraps the stream and emits both `StreamEvent`s (for SSE) and side-effect trace events (via `TraceCollector`).
2. Map each event type:
   - `TextDelta` → emit to SSE; no trace.
   - `ToolUseStart` → record the start time in the translator's state; emit to SSE; no trace yet (the trace fires when the matching `ToolResult` arrives, with the duration).
   - `ToolUseDelta` → emit to SSE; no trace.
   - `ToolResult` → look up the matching `ToolUseStart` by id; compute the duration; record a `tool_call` trace event with `cli_managed=true`, name, sanitized input, sanitized output, duration, is_error; emit to SSE.
   - `Thinking` → emit to SSE; coalesce consecutive `Thinking` events into a single `decision` trace event with the concatenated text and `source=claude_code` (per the feat-022 "decision fragmentation" fix in PROGRESS.md).
   - `Error` → emit to SSE; record an `error` trace event.
   - `Done` → emit to SSE; no trace.
   - `FileChangeAnnouncement` (CLI-specific) → record a `file_change` row directly (the action is read/write/create/delete).
3. Handle orphaned tool uses. If a `ToolUseStart` arrives but no matching `ToolResult` within the same turn (the CLI dropped it), record a `tool_call` trace event with `status="orphaned"` and emit an `error` SSE event for the journey sidebar.
4. Dedupe file changes. If the same path is announced multiple times in one turn, the `file_changes` table keeps only the most recent action (existing behavior from feat-017; the translator doesn't need to do anything special).
5. Add tests. Each mapping is a test. The orphan case is a test. The dedup is a test.

**Acceptance criteria:**
- [ ] `test_journey_translator_text_passthrough` — `TextDelta` is forwarded; no trace event.
- [ ] `test_journey_translator_tool_call_recorded` — `ToolUseStart` + `ToolResult` produces one `tool_call` trace event with the right name, input, output, duration, and `cli_managed=true`.
- [ ] `test_journey_translator_tool_not_re_executed` — assert that the translator NEVER calls `ToolRegistry`; the trace event's output is the CLI's output verbatim (modulo sanitization).
- [ ] `test_journey_translator_file_change_recorded` — a `FileChangeAnnouncement` produces a `file_changes` row with the right path, action, and session id.
- [ ] `test_journey_translator_thinking_to_decision` — three consecutive `Thinking` events produce one `decision` trace event with the concatenated text and `source=claude_code`.
- [ ] `test_journey_translator_error_to_error` — an `Error` event produces an `error` trace event with the right message.
- [ ] `test_journey_translator_orphaned_tool_use` — a `ToolUseStart` with no matching `ToolResult` produces an `error` trace event and a `tool_call` with `status="orphaned"`.
- [ ] `test_journey_translator_dedupes_file_changes` — three `FileChangeAnnouncement` events for the same path with actions write→write→create produce one `file_changes` row with action=create (most recent wins).
- [ ] `test_journey_translator_sanitizes_secrets` — a tool input containing a key matching `secret` has its value stripped from the trace event.
- [ ] All existing journey and trace tests pass (the native-mode trace shape is unchanged).

**Verification:**
```bash
cargo test -p weave-server -- test_journey_translator
./init.sh
```

**Design decisions already made:**
- The translator does NOT re-execute tools. This is the core promise of "wrapped mode is `wrapped`, not `delegated`." The CLI ran the tool; we record what it did.
- The translator is stateless across turns. Each turn is a fresh translator instance. (Within a turn, it has state for in-flight tool calls; across turns, no state.)
- `cli_managed=true` is the marker that distinguishes CLI-originated traces from Weave-executed traces (the latter is the feat-037 native tool loop). The journey sidebar in feat-022 may want to render this; not a hard requirement for this feature.

**Design decisions open:**
- Whether `FileChangeAnnouncement` is an event in the parser's output (this feature) or a side channel (the runner detects file changes by watching the filesystem). The strategy doc says the CLI emits them; if Claude Code does, the parser path is right. If not, the filesystem watcher is the fallback. Decide during implementation; the task description is written for the parser path.
- The exact sanitization rule. Existing feat-017 sanitizes keys matching `secret|key|token|password`; reuse the same rule. Add a config knob in a future feature if users complain.

**Dependencies:** feat-005, feat-017, feat-043, feat-045.

---

### feat-049: Child-process reaping on startup + per-session tracking

**Strategic context:** §6 of the strategy: "Child-process reaping on crash. Normal cancel kills the tracked child process. Startup also scans for surviving CLI processes associated with inactive sessions; database-only orphan reaping is not enough once wrapped CLIs exist." This is the operational safety net for the new lifecycle.

**Goal:** A per-session process table that maps `session_id` to the active child handle. Cancel routes SIGTERM to the tracked process. Startup scans the filesystem for surviving CLI processes whose parent pid is Weave's pid and whose argv matches a known CLI binary, then terminates them.

**Data shape changes:**
- New `ActiveChildProcesses` type, parallel to the existing `ActiveSessions` from feat-034. Key: `SessionId`. Value: `(Pid, Child)`. The runner (feat-043) registers a child on spawn and deregisters on exit. Cancel consults the table.
- New `reap_cli_processes` startup hook. Reads `/proc` on Linux, finds children of the Weave pid whose `comm` matches a known CLI binary basename (e.g., `claude`, `codex`, `opencode`), checks the session id in the child's env (`WEAVE_SESSION_ID` injected by the runner), and terminates them.
- New config option: `WEAVE_REAP_CLI_PROCESSES_ON_STARTUP` (default `true`). Some users may want to disable this in dev.

**Implementation outline:**

1. Add the `ActiveChildProcesses` table. Use `tokio::sync::Mutex<HashMap<SessionId, ChildHandle>>`. The handle includes the pid and a `tokio::process::Child` (or a pid + a oneshot sender for kill).
2. Wire the runner (feat-043) to register / deregister. The runner is already per-turn, so registration happens at spawn and deregistration at exit. The cancel path is what the table is for: when `SessionService::cancel_session` is called, it looks up the child and sends SIGTERM (the runner's cancel path from feat-043 is reused).
3. Add `reap_cli_processes` as a startup hook. Place it AFTER `reap_orphans` (which is the existing session-row reaper from feat-034) and BEFORE the HTTP server starts. The hook:
   - Walks `/proc/*/status` and finds processes whose `PPid` matches Weave's pid.
   - Filters on `comm` matching a known CLI binary basename (loaded from registered CLI providers).
   - Reads `/proc/<pid>/environ` to find `WEAVE_SESSION_ID=<id>`.
   - Looks up the session id in the DB; if the session is terminal (completed/cancelled/error) OR missing, the process is an orphan and is terminated (SIGTERM, then SIGKILL after 5s).
   - Logs the count of reaped processes.
4. Add a config flag to disable the reaper. Useful for debugging.
5. Tests: a real CLI is not in CI. Tests use a long-running `/bin/sleep` spawned as a child of the test process; the test asserts the reaper terminates it. The CLI-binary filter is tested by passing `WEAVE_REAPER_CLI_BINARIES=foo,bar` and asserting a process named `foo` is reaped while a process named `baz` is not.
6. The cancel-kills-tracked path is a unit test on the runner / `ActiveChildProcesses` interaction.

**Acceptance criteria:**
- [ ] `test_cli_reap_orphan_processes_terminates` — spawn a `/bin/sleep 30` as a child of the test, call `reap_cli_processes` (with the test process's pid and `WEAVE_SESSION_ID=<id>` for a session whose DB row is `error`), assert the child is dead within 1s.
- [ ] `test_cli_cancel_kills_tracked_process` — `SessionService::cancel_session` for a session with a running child sends SIGTERM to the child; the child exits within 1s.
- [ ] `test_cli_reap_idempotent` — calling `reap_cli_processes` twice in a row is safe; the second call reaps nothing.
- [ ] `test_cli_reap_unrelated_process_untouched` — a child process whose `comm` is not in the known CLI list is NOT reaped. (E.g., `/bin/sleep` is reaped if the config lists `sleep`; `/bin/echo` is not reaped.)
- [ ] `test_cli_reap_runs_in_startup_sequence` — the reaper runs in the right order relative to `reap_orphans` and the HTTP server start. Assert by logging the sequence and asserting order in the test.
- [ ] `test_cli_reap_disabled_by_config` — with `WEAVE_REAP_CLI_PROCESSES_ON_STARTUP=false`, the reaper is a no-op (the config flag works).
- [ ] All existing feat-034 graceful-shutdown tests pass unchanged.

**Verification:**
```bash
cargo test -p weave-server -- test_cli_reap
cargo test -p weave-server -- test_graceful_shutdown
./init.sh
```

**Design decisions already made:**
- The reaper uses `/proc` on Linux. Weave is Linux-first; macOS support is a separate concern (and `/proc` is Linux-only). Document the limitation in the feature PR.
- The CLI-binary filter is a config-supplied list, not a hardcoded list. Default to an empty list (no reaping) for the feat-049 ship; Phase 8's feat-051 will populate the list with the registered CLI binaries at startup.
- The reaper only acts on processes that have a `WEAVE_SESSION_ID` env var. This is a contract: the runner sets the env var on every spawn. Without the env var, the process is not associated with a session and is left alone.

**Design decisions open:**
- The cleanup behavior on macOS. Out of scope for v1; document and revisit.
- Whether the reaper should be triggered on a timer (e.g., every 5 minutes) in addition to startup. Decision: no for v1. A timer adds complexity; startup is sufficient for the strategy's intent.

**Dependencies:** feat-009, feat-034, feat-043.

---

### feat-050: Workspace-scoped CLI session validation

**Strategic context:** §8 of the strategy: "Workspace-scoped CLI sessions. Wrapped CLI sessions must run inside a registered `Codebase` row. The session creation modal should reject wrapped sessions whose `cwd` is outside a registered codebase because kanban auto-spawn and trace storage both rely on workspace-scoped codebase context." This feature is the enforcement.

**Goal:** Session creation with `mode=wrapped` + a CLI runtime kind requires `cwd` to be inside a registered `Codebase` row for the workspace. Reject at session creation with a clear error. Kanban auto-spawn and A2A enforce the same check.

**Data shape changes:**
- New error variant: `AppError::Validation` with `code: "cwd_outside_codebase"`, `details: { cwd: String, registered_codebases: Vec<{id, path, label}> }`. The list lets the UI suggest where the user should have pointed.
- The session create handler gains the check. So does `try_automate_lane` and A2A.
- For native (HTTP) sessions, the check is NOT applied. Native sessions do not need to be inside a registered codebase (e.g., a user can have a Claude session for a `/tmp` scratch space).

**Implementation outline:**

1. Add the validation function `validate_wrapped_cwd_in_codebase(workspace_id, runtime_kind, mode, cwd) -> Result<CodebaseId, AppError>`. Uses `dunce::canonicalize` for path normalization (handles Windows quirks even though we are Linux-first; cheap and forward-compatible).
2. Wire it into session create, kanban auto-spawn, and A2A message send.
3. For kanban auto-spawn: if a card is moved to a column with `auto_trigger=true`, the auto-spawn must resolve a `cwd` (from the column's `cwd` field, the board's default, or the workspace's default) and validate it. If validation fails, the auto-spawn fails; the card stays in the column; an SSE `session_start_failed` event is broadcast.
4. For A2A: the same path. If validation fails, the A2A request returns the structured error and no session is created.
5. Tests cover all three call sites plus the negative path (cwd outside any codebase).

**Acceptance criteria:**
- [ ] `test_wrapped_session_requires_codebase` — `POST /api/workspaces/:wid/sessions` with `runtime_kind=claude-code, mode=wrapped, cwd=/some/path` and NO registered codebases returns `code: "cwd_outside_codebase"` with the list empty.
- [ ] `test_wrapped_session_cwd_outside_codebase_rejected` — cwd is `/foo` and there is a registered codebase at `/bar`; the create returns the structured error with the registered codebase in the details.
- [ ] `test_wrapped_session_cwd_inside_codebase_accepted` — cwd is `/bar/sub` and the registered codebase is `/bar`; the create succeeds; the session is created with `codebase_id` set to the matching codebase.
- [ ] `test_native_session_no_codebase_requirement` — `runtime_kind=anthropic-api, mode=native, cwd=/tmp/foo` succeeds even with no registered codebases.
- [ ] `test_kanban_wrapped_autospawn_validates_cwd` — moving a card to a wrapped auto-trigger column with an invalid cwd broadcasts `session_start_failed` and does NOT create a session.
- [ ] `test_kanban_wrapped_autospawn_succeeds_with_valid_cwd` — same scenario, valid cwd; session is created.
- [ ] `test_a2a_wrapped_session_validates_cwd` — `POST /api/a2a/messages` with `runtime_kind=claude-code, mode=wrapped` and invalid cwd returns the structured error.
- [ ] `test_wrapped_session_path_canonicalization` — `cwd=/foo/./bar/../bar/sub` and codebase at `/foo/bar`; the validation succeeds (path is canonicalized before comparison).

**Verification:**
```bash
cargo test -p weave-server -- test_wrapped_session
cargo test -p weave-server -- test_kanban_wrapped_autospawn
cargo test -p weave-server -- test_a2a_wrapped_session
./init.sh
```

**Design decisions already made:**
- The check is at session creation, not at prompt time. A user could theoretically change a `cwd` between creates; we re-validate each time.
- Native sessions are explicitly excluded from the check. Document this; users may want a one-off native session for `/tmp` work.
- The error includes the list of registered codebases. This is friendlier than just saying "cwd is invalid" and lets the UI suggest a fix.

**Design decisions open:**
- Whether a session can have `cwd` outside a codebase but be associated with a codebase via an explicit `codebase_id` field (not just cwd inference). The data model already has `codebase_id` on `sessions` (per `docs/data-model.md`); this feature uses it. Confirm the migration adds a column or uses the existing one.
- The behavior when a codebase is DELETED while a session is still active. The session's `cwd` may no longer be in any codebase. The check is at create time, so this is moot for new sessions. Existing sessions continue; they just can't be resumed (because the create-time check would now fail). Document.

**Dependencies:** feat-008, feat-032, feat-040.

---

### feat-051: `ClaudeCodeCodingAgent` end-to-end (fake harness)

**Strategic context:** §3 of the strategy: "Build the CLI subprocess harness, fake CLI test fixture, stream parser, permission mapper, runtime session-id capture, resume behavior, cancellation behavior, and journey translation around Claude Code." Every prior Phase 8 feature is a piece of that list. This feature wires them all into the first working `CliCodingAgent` impl and runs the full lifecycle end-to-end against the fake CLI.

**Goal:** A `ClaudeCodeCodingAgent` that implements `CodingAgent` (with the `TurnContext` signature from feat-041) and is registered in the `ProviderRegistry` for `kind=cli, binary=<claude>`. End-to-end tests use the fake CLI harness. A real `claude` binary is a manual smoke check. Anthropic API in native mode stays green.

**Data shape changes:**
- New `ClaudeCodeCodingAgent` type in `agent::adapters::claude_code`. Implements `CodingAgent`. Holds references to the runner (feat-043), the parser (feat-045), the translator (feat-048), and the permission mapper (feat-046).
- New `ProviderRegistry::add_provider` branch for `kind=cli` whose `binary_path` resolves to the `claude` basename. Constructs a `ClaudeCodeCodingAgent`. (feat-039's "NotImplemented" error is removed for the Claude Code case; it remains for other CLI kinds until Phase 10.)
- The resume flag (`--resume <id>`) is added by this adapter to the argv built by the runner. The runner does not know about it.
- The `done` event includes `runtime_kind="claude-code", mode="wrapped", resume_state=<state>`.

**Implementation outline:**

1. Implement `ClaudeCodeCodingAgent::send_message(request, turn)`. The flow:
   - Look up the adapter-specific config from the provider row (`binary_path`, `args`, `env`, `permission_mode`).
   - Build a `PermissionSnapshot` via the `ClaudeCodePermissionMapper` (feat-046) using `turn.effective_permissions.profile` (or the session's profile — confirm).
   - Build the `CliInvocation`: `binary=binary_path`, `args=[...cli_flags_from_snapshot, "--resume", "<turn.cli_resume_id>", ...]`, `env=[...env_from_snapshot, "WEAVE_SESSION_ID"=<session_id>]`, `cwd=turn.cwd`.
   - Call `CliRunner::run(invocation, turn)`. Get back a `LineStream` and a `CliRunResult`.
   - Pipe the `LineStream` through `ClaudeCodeStreamParser` (feat-045). Get back a `Stream<StreamEvent>`.
   - Pipe that through `JourneyTranslator` (feat-048). Get back a `Stream<Result<StreamEvent, AppError>>` that the `SessionService` can consume.
   - On successful exit, persist the captured session id into `runtime_metadata_json['cli_resume_id']` (feat-047). On `resume_unknown_session` error from the CLI, clear the id and re-invoke without the resume flag (feat-047's fallback).
2. Register the agent in `ProviderRegistry::add_provider`. The dispatch key is the binary basename (`claude`). Other CLIs (Phase 10) register with their own basenames.
3. Update `ProviderRegistry::add_provider`'s `kind=cli` branch: if the binary basename is `claude`, build a `ClaudeCodeCodingAgent`; otherwise return the existing `NotImplemented` error. (Phase 10's features will add the other branches.)
4. Add end-to-end tests using the fake CLI harness. The tests:
   - Register a `kind=cli` provider whose `binary_path` points to the compiled `fake_cli` binary with `FAKE_CLI_SCRIPT=text+tool+done`.
   - Create a session with `runtime_kind=claude-code, mode=wrapped, cwd=<some codebases subdir>`.
   - Send a prompt. Assert the SSE stream has `TextDelta`, `ToolUseStart`, `ToolUseDelta`, `ToolResult`, `Done`. Assert the journey has a `tool_call` event.
   - Send a second prompt. Assert the resume id is passed to the CLI. Assert the fake's `session_id` is echoed back.
   - Cancel mid-stream. Assert the child process is reaped.
5. The real-CLI smoke test is documented in a `docs/user/sessions.md` section ("Running Claude Code wrapped mode") and a manual test script in `scripts/smoke_claude_code.sh`. It is NOT a CI gate.

**Acceptance criteria:**
- [ ] `test_claude_code_wrapped_session_create` — create a session with the fake CLI registered; assert the session is created with the right `runtime_kind, mode, provider_id`.
- [ ] `test_claude_code_wrapped_streams_via_sse` — send a prompt; assert the SSE stream has the expected event sequence (driven by the fake's `text+tool+done` script).
- [ ] `test_claude_code_wrapped_resume_first_turn_native_second` — first turn `resume_state="none"`; second turn `resume_state="native"`; assert via the SSE `done` events.
- [ ] `test_claude_code_wrapped_cancel_mid_stream` — cancel a long-running turn; assert `Done { stop_reason: Cancelled }` and the child process is reaped within 1s.
- [ ] `test_claude_code_wrapped_falls_back_to_replay` — second turn with the fake configured to emit `resume_unknown_session`; assert the runner replays and `resume_state="replayed"`.
- [ ] `test_claude_code_wrapped_records_journey` — after a turn, `/api/sessions/:sid/trace/journey` returns a `tool_call` event with `cli_managed=true` and the right name, input, output, duration.
- [ ] `test_native_anthropic_still_passes_through_loop` — explicit regression: an Anthropic API session with a tool call completes the loop and the journey is recorded. The existing Anthropic API tests still pass.
- [ ] `test_claude_code_wrapped_cwd_validation` — wrapped session with cwd outside a registered codebase returns the `cwd_outside_codebase` error.
- [ ] All existing tests pass; `./init.sh` is green.

**Verification:**
```bash
cargo test -p weave-server -- test_claude_code_wrapped
cargo test -p weave-server -- test_native_anthropic_still_passes_through_loop
./init.sh
```

**Design decisions already made:**
- The fake CLI is the conformance target. Real `claude` is a manual smoke check only.
- The adapter adds the resume flag to argv; the runner does not know it. This keeps the runner generic.
- The adapter registers by binary basename (`claude`). Other CLIs (Codex, OpenCode) register by their own basenames in Phase 10.

**Design decisions open:**
- Whether to add a `#[ignore]`-marked integration test that hits the real Anthropic API with a real Claude Code CLI binary. The test is slow, requires credentials, and is flaky. The answer is almost certainly no — but flag in the PR for discussion.
- The smoke-test script in `scripts/smoke_claude_code.sh` is a developer convenience. Decide if it should live in the repo or in a separate ops repo.

**Dependencies:** feat-037, feat-038, feat-039, feat-040, feat-041, feat-042, feat-043, feat-044, feat-045, feat-046, feat-047, feat-048, feat-049, feat-050.

---

## Phase 9 — Multi-Runtime User Surface

### feat-052: Settings page Runtime Tool-aware form

**Strategic context:** §5 of the strategy: "The UI presents each registered row as a Runtime Tool." §7: "The Settings page replaces its hardcoded `type: 'anthropic'` with a Runtime Tool-aware form, even if the backend endpoints remain named `/api/providers` during the first implementation." The Settings page is the user's primary place to register a CLI binary before they can use it in a session.

**Goal:** The provider list shows a `kind` badge (HTTP / CLI) on each row. The "Add provider" modal renders a kind picker first; once chosen, the rest of the form is the variant-specific shape. HTTP form fields: name, base_url, api_key, default_model. CLI form fields: name, binary_path, args, env, default_model, permission_mode. Existing providers (kind='http') keep rendering as today.

**Data shape changes:** No new backend types. The frontend reads the existing `GET /api/providers` response (which now includes `kind`, `binary_path`, `args_json`, `env_json`, `permission_mode` from feat-039) and renders the matching form.

**Implementation outline:**

1. Audit the existing `web/src/app/pages/Settings.tsx` and the `AddProviderModal` component. Identify the hardcoded `type: "anthropic"` shape.
2. Add a `kind` step to the modal. The step is a radio group: HTTP / CLI. The rest of the form is conditional.
3. For HTTP: keep the existing fields. The submit body uses `kind: "http"` and the existing field names.
4. For CLI: render name, binary_path (with a "use absolute path" hint and a small validation that the path starts with `/`), args (a dynamic list of strings — `+` button to add, `×` to remove), env (a key/value list), default_model (free text + datalist populated from `GET /api/providers/:id/models` if available), permission_mode (a preset dropdown with the strategy's values + "custom" for free text).
5. Update the provider list rows to show a `kind` badge. Use the same badge style as the existing status badges in the codebase.
6. Update the model list to use the cached list from feat-042 (the `cached_at` is shown as a small "updated 3 min ago" text in the row).
7. Tests: add component tests for both form variants. Use `web/src/test-utils` helpers; the existing Settings tests from feat-020 are the template.

**Acceptance criteria:**
- [ ] `web/src/app/pages/Settings.test.tsx` — adding an HTTP provider works as today (the existing flow is unchanged).
- [ ] `web/src/app/pages/Settings.test.tsx` — adding a CLI provider submits with `kind: "cli"` and the CLI-specific fields; the success toast fires; the new row appears in the list with the CLI badge.
- [ ] `web/src/app/pages/Settings.test.tsx` — the model datalist in the CLI form is populated from `GET /api/providers/:id/models` (mock the endpoint).
- [ ] `web/src/app/pages/Settings.test.tsx` — submitting with a missing `binary_path` shows the inline validation error.
- [ ] All existing Settings tests pass unchanged.
- [ ] `bun run lint`, `bun run typecheck`, `bun run build` are green.

**Verification:**
```bash
cd web && bun run test -- --run settings-runtime-tool
cd web && bun run test
cd web && bun run lint
cd web && bun run typecheck
cd web && bun run build
```

**Design decisions already made:**
- The frontend does NOT pre-validate that `binary_path` exists. The backend's soft-WARN behavior (feat-039) is the source of truth; pre-validation on the frontend would race against install/uninstall.
- The `args` and `env` lists are simple `string[]` and `Record<string, string>` shapes; no nesting, no JSON editing.

**Design decisions open:**
- Whether to add a "Test connection" button to the form that runs the existing `/api/health`-like check against the provider. Out of scope for this feature; consider for a future UX improvement.
- Whether the model datalist should show the model's `context_window` from `ModelInfo`. Nice-to-have; defer if it requires backend changes.

**Dependencies:** feat-020, feat-039, feat-042.

---

### feat-053: 4-step session creation sheet

**Strategic context:** §7 of the strategy: "The interaction model has four steps in this fixed order, both for human-driven and kanban-driven session creation: (1) Runtime Tool — which engine runs the session. (2) Role — which system prompt the agent runs under (specialist). (3) Model — which model within the Runtime Tool. (4) What it works on — workspace, registered codebase, optional kanban task." This feature is the human-driven path.

**Goal:** Replace the current 3-field "New Session" modal with a 4-step wizard. Each step has a Back button. Validation at each step. The wizard is a slide-over (the existing `TaskDetailPanel` from feat-026 is the visual template), not a centered modal.

**Data shape changes:**
- The `POST /api/workspaces/:wid/sessions` body gains two optional fields: `runtime_kind` (string) and `mode` (string, defaults based on `runtime_kind`). Existing call sites that omit them get the HTTP/native default. The `validate_runtime_mode_compat` check (feat-040) runs server-side.
- The wizard's step 1 lists providers from `GET /api/providers`, filtered to those with healthy `health_check` results. The frontend's filter is a soft check; the backend is the source of truth.

**Implementation outline:**

1. Audit the existing `web/src/app/pages/WorkspaceDetail.tsx` (or wherever the "New Session" modal lives) and the existing modal component.
2. Replace the modal with a 4-step wizard. Each step is a slide in the same sheet (the slide changes with an animation; the user does not leave the sheet).
3. Step 1: Runtime Tool. List providers; for each, show name, kind badge, and a small "healthy" / "unhealthy" indicator. Selecting a provider advances to step 2.
4. Step 2: Role. List specialists (existing endpoint). Filter to specialists whose `tool_profile` is compatible with the chosen Runtime Tool (the matrix from feat-040; the frontend has a copy of the table for UX, but the server is the source of truth). Selecting a role advances to step 3.
5. Step 3: Model. List models from `GET /api/providers/:id/models` (the cache from feat-042). First model is pre-selected; the user can change. Advancing to step 4.
6. Step 4: workspace target (default current workspace, not editable for now — multi-workspace is a separate concern), codebase (filtered to the workspace's codebases, required for `mode=wrapped` per feat-050), kanban task (optional, filtered to active tasks not currently being worked on). Submit button creates the session.
7. Validation: each step has a "Next" button that runs a local validation. The final Submit calls `POST /api/workspaces/:wid/sessions` with the assembled body. On 4xx, the wizard scrolls to the failing step and shows the error inline.
8. The backend handler (existing) gains the `runtime_kind` / `mode` fields and the `validate_runtime_mode_compat` check. The provider_id is resolved from the chosen runtime kind (the chosen provider IS the runtime).
9. Tests: the wizard has a small set of unit tests (component rendering, step transitions) and a smoke test (mock the endpoints, fill the wizard, assert the submit body). The full happy path is a manual check; a Playwright e2e is a future improvement.

**Acceptance criteria:**
- [ ] `web/src/app/pages/NewSessionWizard.test.tsx` — the wizard renders 4 steps in order. Back works from each step.
- [ ] `web/src/app/pages/NewSessionWizard.test.tsx` — step 1 lists providers; selecting one advances to step 2.
- [ ] `web/src/app/pages/NewSessionWizard.test.tsx` — step 2's role list is filtered to compatible roles for the chosen runtime.
- [ ] `web/src/app/pages/NewSessionWizard.test.tsx` — step 3's model list comes from the chosen provider's `list_models` endpoint.
- [ ] `web/src/app/pages/NewSessionWizard.test.tsx` — step 4 requires a codebase for `mode=wrapped`; the input is enforced.
- [ ] `web/src/app/pages/NewSessionWizard.test.tsx` — submitting assembles the right body and POSTs to the session create endpoint.
- [ ] `web/src/app/pages/NewSessionWizard.test.tsx` — a 4xx from the backend scrolls to the failing step and shows the error.
- [ ] `cargo test -p weave-server -- test_session_create_with_runtime_kind` — the backend accepts the new fields and validates the matrix.
- [ ] All existing session creation tests pass.

**Verification:**
```bash
cd web && bun run test -- --run new-session-wizard
cd web && bun run test
cd web && bun run lint
cd web && bun run typecheck
cd web && bun run build
cargo test -p weave-server -- test_session_create_with_runtime_kind
./init.sh
```

**Design decisions already made:**
- The wizard is a slide-over, not a modal. Matches the existing `TaskDetailPanel` pattern from feat-026.
- The wizard does NOT support multi-workspace session creation in v1. The workspace is the current workspace.
- The role list is filtered client-side using a copy of the matrix. The server is the source of truth for the final validation; the client filter is a UX nicety.

**Design decisions open:**
- Whether the wizard should remember the last choice (per user, persisted in `localStorage`). Nice-to-have; out of scope for this feature.
- Whether the model selection should support "use provider default" (i.e., the user skips step 3 and the provider's `default_model` is used). This is a backend concern; the existing POST allows `model` to be `null`. Confirm the wizard's UI.

**Dependencies:** feat-021, feat-040, feat-041, feat-042.

---

### feat-054: Session page layout switcher (native / wrapped / attended)

**Strategic context:** §5 of the strategy: "Attended mode is *not* in the `CodingAgent` trait. It is a different lifecycle (long-lived subprocess, user-driven, not model-driven). It is a separate `Terminal` abstraction that the session page renders, parallel to `CodingAgent`." §7: "The session page itself renders one of three layouts (`native` / `wrapped` / `attended`) from the same `/session/:id` URL." This feature is the layout switcher minus the attended layout (which is Phase 11).

**Goal:** The session page renders one of two layouts today (the third, `attended`, is rejected at session creation until Phase 11). The `wrapped` layout is the existing chat view with a header pill showing Runtime Tool + active permission mode + resume state. The `native` layout is the existing chat view unchanged.

**Data shape changes:** No new backend types. The session page reads `runtime_kind`, `mode`, and the `done` event's `resume_state` (from feat-047).

**Implementation outline:**

1. Audit `web/src/app/pages/SessionPage.tsx` and the `SessionHeader` component.
2. Add a `WrappedSessionBanner` component. It renders above the message list on the first turn of a wrapped session with plain-language text: "You're using Claude Code (wrapped mode). The CLI runs the tools; Weave watches the journey." The banner is dismissible per-session (state in `useState`).
3. Extend `SessionHeader` to render a pill row when `mode === "wrapped"`. The pill row has: Runtime Tool display name, the active permission mode (from the most recent SSE event's `PermissionSnapshot`, captured into `useState` per turn), the resume state (`none` / `native` / `replayed`) from the most recent `done` event.
4. Add the layout switch. The simplest implementation: the existing `SessionPage` body is the `native` layout. When `mode === "wrapped"`, the body is the same but `SessionHeader` and `WrappedSessionBanner` render the wrapped bits. The `attended` layout is a placeholder component that returns "Attended mode is not yet available" (until Phase 11).
5. The 'jump to latest' pill and SSE streaming behavior from feat-036 are unchanged.
6. Tests: component tests for the new banner and the header pill. Mock the SSE stream with a `done` event that has a `resume_state`. The existing session chat tests (feat-021, feat-036) must still pass.

**Acceptance criteria:**
- [ ] `web/src/app/pages/SessionPage.test.tsx` — a `mode=native` session renders the existing chat view with no banner.
- [ ] `web/src/app/pages/SessionPage.test.tsx` — a `mode=wrapped` session renders `WrappedSessionBanner` above the messages on the first turn; the banner is dismissible.
- [ ] `web/src/app/pages/SessionPage.test.tsx` — `SessionHeader` for a `mode=wrapped` session shows the Runtime Tool pill, the permission mode pill, and the resume state pill.
- [ ] `web/src/app/pages/SessionPage.test.tsx` — the `mode=attended` session (synthesized via mock data) renders the "not yet available" placeholder.
- [ ] `web/src/app/pages/SessionPage.test.tsx` — the `resume_state` updates from `none` → `native` between the first and second `done` events.
- [ ] All existing session chat tests pass unchanged.
- [ ] `bun run lint`, `bun run typecheck`, `bun run build` are green.

**Verification:**
```bash
cd web && bun run test -- --run session-layouts
cd web && bun run test
cd web && bun run lint
cd web && bun run typecheck
cd web && bun run build
```

**Design decisions already made:**
- The `wrapped` and `native` layouts share 95% of components. The differences are isolated to `SessionHeader` and `WrappedSessionBanner`.
- The attended layout is a placeholder for now. The slot is reserved so feat-060 can drop in the terminal pane without changing the layout switcher.

**Design decisions open:**
- Whether the `WrappedSessionBanner` should be a permanent fixture of wrapped sessions (always visible) or a first-time-only onboarding hint. Decision: dismissible per-session. The user can re-show it from a help menu (future feature).
- The exact wording of the banner. The strategy doc does not specify; the implementer can iterate based on user feedback.

**Dependencies:** feat-021, feat-040, feat-051.

---

### feat-055: Kanban column `(runtime_kind, specialist_id)` binding

**Strategic context:** §5 of the strategy: "A2A and kanban must stop silently choosing the first Provider once Runtime Tools are explicit. Multi-runtime implementation must add explicit Tool/provider selection for new A2A requests and kanban column bindings." This is the kanban half.

**Goal:** Columns gain a nullable `runtime_kind` field. Auto-spawned sessions use the column's binding. If `runtime_kind` is null, behavior is unchanged. The frontend shows a Runtime Tool badge on auto-spawn columns. The silent first-provider fallback is removed from the kanban path.

**Data shape changes:**
- New migration `resources/migrations/012_columns_runtime_kind.sql`:
  ```sql
  ALTER TABLE columns ADD COLUMN runtime_kind TEXT;
  ```
- Update the `columns` CREATE TABLE block in the bootstrap migration.
- `Column` struct gains `runtime_kind: Option<RuntimeKind>`. SELECT / INSERT / UPDATE in `KanbanStore` include the new column.
- `POST /api/boards/:bid/columns` and `PATCH /api/columns/:cid` accept `runtime_kind`.
- `try_automate_lane` resolves the session's `runtime_kind` from the column's binding (with workspace default as fallback). If the resolved `runtime_kind` is not present and no workspace default exists, `try_automate_lane` returns a clear error and the auto-spawn fails (no silent first-provider fallback).

**Implementation outline:**

1. Add the migration. Update the bootstrap migration's CREATE TABLE.
2. Add the field to `Column`. Update SELECT / INSERT / UPDATE in `KanbanStore`. The `FromRow` mapping includes the new field.
3. Update the API handlers for column create and update to accept the field.
4. Update `try_automate_lane` to resolve and use the binding.
5. Update the workspace config to have a `default_runtime_kind` field (the workspace default). New env var `WEAVE_DEFAULT_RUNTIME_KIND` with default `anthropic-api`. The migration adds the column to the `workspaces` table? Or to a new `workspace_config` table? The simpler path: a new `workspace_settings` table (key-value per workspace) for `default_runtime_kind`. This avoids widening the `workspaces` table for a single config field.
6. Frontend: `ColumnHeader` shows a small Runtime Tool badge when `runtime_kind` is set and `auto_trigger` is on. The badge is the same `kind` badge style as feat-052.
7. Tests: backend unit + integration tests for the binding logic. Frontend component tests for the badge.

**Acceptance criteria:**
- [ ] `test_columns_migration_adds_runtime_kind` — the new column is in the schema; existing rows have `runtime_kind=NULL`.
- [ ] `test_kanban_autospawn_uses_column_runtime_kind` — column with `runtime_kind=claude-code, specialist_id=dev-crafter, auto_trigger=true`; move a card in; the auto-spawned session has `runtime_kind=claude-code, mode=wrapped, specialist_id=dev-crafter`.
- [ ] `test_kanban_autospawn_inherits_when_null` — column with `runtime_kind=NULL, auto_trigger=true`; workspace default is `anthropic-api`; auto-spawned session uses `anthropic-api`.
- [ ] `test_kanban_autospawn_errors_when_no_default` — column with `runtime_kind=NULL, auto_trigger=true`; workspace has no `default_runtime_kind`; auto-spawn fails with a clear error pointing at the column's binding.
- [ ] `test_kanban_autospawn_runtime_mode_compat_check` — column with `runtime_kind=claude-code, mode=native` (incompatible); auto-spawn fails with the `runtime_mode_incompatible` error from feat-040.
- [ ] `test_workspace_default_runtime_kind` — `WEAVE_DEFAULT_RUNTIME_KIND=claude-code` is read at startup; new workspaces have the right default; existing workspaces pick up the default.
- [ ] Frontend: `web/src/app/pages/BoardContainer.test.tsx` — columns with `runtime_kind` show the badge; columns without do not.
- [ ] All existing kanban tests pass unchanged.

**Verification:**
```bash
cargo test -p weave-server -- test_columns_migration_adds_runtime_kind
cargo test -p weave-server -- test_kanban_autospawn
cargo test -p weave-server -- test_workspace_default_runtime_kind
cd web && bun run test -- --run kanban-runtime-badge
cd web && bun run test
./init.sh
```

**Design decisions already made:**
- The workspace default lives in a new `workspace_settings` table, NOT as a column on `workspaces`. The `workspaces` table is small and stable; per-workspace config grows over time and shouldn't bloat the row.
- Removing the first-provider fallback is part of THIS feature, not deferred. The strategy doc is explicit: "Multi-runtime replaces it" (§6). If a user had been relying on the first-provider fallback, the breakage is intentional and visible in the changelog.

**Design decisions open:**
- The behavior when the column's `runtime_kind` references a CLI runtime kind whose binary is not registered. Current plan: auto-spawn fails with a clear error. The user must either (a) register the provider, (b) change the column's binding, or (c) set a workspace default. Document.
- Whether the column's `specialist_id` should also be required when `runtime_kind` is set. Decision: keep both independent. A column can have a `runtime_kind` but no specialist (the system default prompt is used). This matches existing behavior.

**Dependencies:** feat-024, feat-025, feat-040.

---

### feat-056: A2A explicit Runtime Tool selection

**Strategic context:** §5 of the strategy: "A2A and kanban must stop silently choosing the first Provider once Runtime Tools are explicit." §8: "A2A exposure of wrapped sessions. External A2A callers may specify a Runtime Tool per request when the user has permitted it. Existing A2A callers that omit it inherit the session's current binding or an explicit configured default. The current first-provider fallback must be replaced before multi-runtime A2A is considered complete."

**Goal:** A2A `POST /api/a2a/messages` accepts an optional `runtime_kind` on the request. Resolution order: request body → session's current runtime (if resuming) → explicit configured default. The first-provider fallback is removed. The Agent Card lists the configured default.

**Data shape changes:**
- New env var `WEAVE_A2A_DEFAULT_RUNTIME_KIND` (default `anthropic-api`). Read at startup; stored in the agent card config.
- `POST /api/a2a/messages` body accepts an optional `runtime_kind` field.
- `GET /.well-known/agent.json` (the Agent Card) lists the configured default. (The Card is already built from loaded specialists; add the default field.)
- New error variant for A2A: `code: "no_provider_for_runtime"` listing the runtime and the registered providers for it.
- The A2A `SendMessage` response's `Task` includes the resolved `runtime_kind` and `mode` so the caller knows what was used.

**Implementation outline:**

1. Add the env var and read it at startup. Pass it to the A2A module.
2. Update `POST /api/a2a/messages` to resolve the `runtime_kind`:
   - If the body has `runtime_kind`, use it (after `validate_runtime_mode_compat` from feat-040).
   - Else if the request is resuming an existing session, use the session's `runtime_kind`.
   - Else use the configured default.
3. Resolve the `provider_id` from the chosen `runtime_kind` via the provider registry. If multiple providers match the kind (e.g., two `kind=cli` providers with `binary_path=claude`), use the first healthy one. (Strategy §8: "The current first-provider fallback must be replaced before multi-runtime A2A is considered complete" — but for providers of the same kind, first-healthy is the only sensible rule. Document.)
4. If no healthy provider matches, return `code: "no_provider_for_runtime"` with the registered providers for the kind.
5. Update the Agent Card to include `default_runtime_kind`.
6. Add tests for each resolution path.
7. Document the breaking change in `CHANGELOG.md` and `docs/api-contracts.md`. The change is: A2A callers that omitted `runtime_kind` and relied on the first-provider fallback will now get the configured default (`anthropic-api` by default). If they want a different runtime, they must specify it.

**Acceptance criteria:**
- [ ] `test_a2a_explicit_runtime_kind` — `POST /api/a2a/messages` with `runtime_kind=claude-code` creates a Claude Code session; the resolved `provider_id` is from a `kind=cli, binary_path=<claude>` provider.
- [ ] `test_a2a_uses_session_runtime_when_resuming` — `POST /api/a2a/messages` with a `context_id` referring to an existing Claude Code session omits `runtime_kind`; the new session uses `claude-code` (the parent's runtime).
- [ ] `test_a2a_uses_configured_default` — `POST /api/a2a/messages` with no `runtime_kind` and no parent session uses the configured default (`anthropic-api` if `WEAVE_A2A_DEFAULT_RUNTIME_KIND` is unset).
- [ ] `test_a2a_no_first_provider_fallback` — assert that the first provider in the list is NOT used; the configured default is used. (Regression guard for the silent fallback.)
- [ ] `test_a2a_errors_when_no_provider_for_runtime` — `runtime_kind=claude-code` but no `kind=cli, binary_path=<claude>` provider is registered; returns `code: "no_provider_for_runtime"` with the registered providers listed.
- [ ] `test_a2a_agent_card_lists_default` — `GET /.well-known/agent.json` includes `default_runtime_kind`.
- [ ] `test_a2a_env_var_override` — `WEAVE_A2A_DEFAULT_RUNTIME_KIND=claude-code` is read at startup; the Agent Card reflects the value; the fallback uses it.
- [ ] `test_a2a_runtime_mode_compat` — A2A request with `runtime_kind=claude-code, mode=native` returns the `runtime_mode_incompatible` error from feat-040.
- [ ] All existing A2A tests pass.

**Verification:**
```bash
cargo test -p weave-server -- test_a2a_explicit_runtime_kind
cargo test -p weave-server -- test_a2a_uses_session_runtime_when_resuming
cargo test -p weave-server -- test_a2a_uses_configured_default
cargo test -p weave-server -- test_a2a_no_first_provider_fallback
cargo test -p weave-server -- test_a2a_errors_when_no_provider_for_runtime
cargo test -p weave-server -- test_a2a_agent_card_lists_default
cargo test -p weave-server -- test_a2a_env_var_override
cargo test -p weave-server -- test_a2a_runtime_mode_compat
cargo test -p weave-server -- test_a2a  # existing
./init.sh
```

**Design decisions already made:**
- The configured default lives in env var, NOT in a per-workspace config. A2A is a single-server concern; multi-workspace A2A defaults are a future feature.
- The first-provider fallback is REMOVED for "any registered provider of any kind." It is REPLACED by "first healthy provider of the resolved kind." This is a deliberate narrowing.

**Design decisions open:**
- Whether the A2A caller should be ABLE to specify a `provider_id` directly (bypassing the kind-based resolution). Decision: no. The strategy doc commits to runtime-kind-based resolution. A caller that wants a specific provider picks a kind that has only one registered provider.
- The behavior when the configured default runtime kind is `claude-code` but no `kind=cli` provider is registered. Decision: fail at startup with a clear error. (A2A requests with no provider for the default would fail at request time anyway; failing at startup is friendlier.)

**Dependencies:** feat-029, feat-040.

---

## Phase 10 — Additional CLI Runtimes

### feat-057: Shared CLI adapter conformance test suite

**Strategic context:** §3 of the strategy: "Codex and OpenCode adapters — added only after the shared CLI adapter contract passes with Claude Code. They should reuse the subprocess runner, parser framework, permission snapshot model, fake CLI harness, and adapter conformance tests." §6 (the conformance suite) is the test scaffold that the next two features depend on.

**Goal:** A conformance test suite in `tests/cli_conformance.rs` that exercises the adapter contract independent of which CLI is being wrapped. Every `CliCodingAgent` impl (Claude Code, Codex, OpenCode) must pass it. The fake CLI harness from feat-044 is the conformance target; real CLIs are not in CI.

**Data shape changes:**
- New test file `crates/weave-server/tests/cli_conformance.rs` (an integration test, not a unit test — it spins up a real `Db` and a real `ProviderRegistry`).
- New trait abstraction (test-only): `ConformanceAdapter` with a method `as_coding_agent(&self) -> &dyn CodingAgent` and a method `fake_cli_binary(&self) -> &Path`. Each adapter (Claude Code, Codex, OpenCode) provides its own test impl of `ConformanceAdapter` that wires the real `CliCodingAgent` to the fake CLI binary.
- The conformance suite iterates the list of `ConformanceAdapter`s. For each, it runs the full set of conformance cases.

**Implementation outline:**

1. Build the `ConformanceAdapter` trait. It is test-only; it lives in `tests/cli_conformance.rs` or a `tests/conformance/mod.rs`.
2. For Claude Code, write a `ClaudeCodeConformanceAdapter` that:
   - Registers a `kind=cli, binary_path=<fake_cli>, name=claude-conformance` provider row.
   - Returns the resulting `Arc<dyn CodingAgent>` from `ProviderRegistry`.
3. For Codex and OpenCode, the conformance adapter is a stub in this feature (feat-058/059 fill them in). The conformance suite SKIPS adapters whose `as_coding_agent()` returns an error. The skip is logged; the test does not fail.
4. Implement the conformance cases. Each case is a function that takes a `&dyn CodingAgent` and a fake CLI script. The cases are listed in the acceptance criteria below.
5. Wire the conformance suite into the verification command. `cargo test -p weave-server --test cli_conformance` is the gate.

**Acceptance criteria (each is a test case in the suite):**
- [ ] `test_conformance_argv_construction` — given a `CliInvocation` with a known `args` and `env`, the adapter builds the right argv for the fake CLI (assert by capturing argv via a small `FAKE_CLI_DUMP_ARGV=1` env var in the fake).
- [ ] `test_conformance_stream_parser` — given a canonical event sequence from the fake, the adapter's parser produces the expected `StreamEvent` sequence.
- [ ] `test_conformance_resume_metadata` — given a session with a persisted `cli_resume_id`, the adapter passes it to the fake on the next turn; the fake echoes it.
- [ ] `test_conformance_permission_mapper` — for each `ToolProfile`, the adapter's `PermissionMapper` produces a `PermissionSnapshot` whose `cli_flags` match the expected list for that adapter.
- [ ] `test_conformance_journey_translator` — given the fake's `text+tool+done` script, the adapter's translator produces a `tool_call` trace event with `cli_managed=true` and the right name / input / output / duration.
- [ ] `test_conformance_error_scenarios` — given the fake's `permission-denied` script, the adapter surfaces an `Error` SSE event. Given the fake's `crash` script, the adapter surfaces an `ExitError` and the session status is `error`.
- [ ] `test_conformance_workspace_scoped_cwd` — the adapter refuses to spawn when `cwd` is outside a registered codebase (the validation from feat-050 is enforced upstream; the conformance test confirms the adapter respects it).
- [ ] `test_conformance_per_turn_process_table` — concurrent sessions for the same adapter do not collide; the active-processes table (feat-049) is consistent.
- [ ] `test_conformance_cancellation` — mid-stream cancel sends SIGTERM to the fake; the fake exits within 1s; the `done` event has `stop_reason=Cancelled`.
- [ ] `test_conformance_loop_limit` — given a fake script that emits `tool_use` indefinitely, the adapter respects the per-turn loop limit (default 8) and exits with `StopReason::LoopLimit`. (This is more relevant for the native loop in feat-037, but the adapter should not infinite-loop either.)

**Verification:**
```bash
cargo test -p weave-server --test cli_conformance
cargo test -p weave-server -- test_conformance_argv_construction
cargo test -p weave-server -- test_conformance_stream_parser
cargo test -p weave-server -- test_conformance_resume_metadata
cargo test -p weave-server -- test_conformance_permission_mapper
cargo test -p weave-server -- test_conformance_journey_translator
cargo test -p weave-server -- test_conformance_error_scenarios
cargo test -p weave-server -- test_conformance_workspace_scoped_cwd
cargo test -p weave-server -- test_conformance_per_turn_process_table
cargo test -p weave-server -- test_conformance_cancellation
cargo test -p weave-server -- test_conformance_loop_limit
./init.sh
```

**Design decisions already made:**
- The conformance suite is integration-level (real DB, real registry, real runner, real parser). It is NOT a unit test that mocks everything. The conformance value comes from running the real plumbing.
- Adapters that are not yet implemented (Codex, OpenCode before their respective features) are skipped with a logged skip, not a failure. The skip becomes a failure once the adapter exists and is registered for the conformance run.

**Design decisions open:**
- Whether to add a benchmark suite (per-adapter cost in tokens / wall time) to the conformance. Out of scope for v1; defer.
- The fake CLI's `FAKE_CLI_DUMP_ARGV=1` mode. Add it as part of feat-057 (small change to the fake); the conformance suite depends on it.

**Dependencies:** feat-043, feat-044, feat-045, feat-046, feat-047, feat-048, feat-050.

---

### feat-058: `CodexCodingAgent` adapter

**Strategic context:** §3 of the strategy: "Codex and OpenCode adapters — added only after the shared CLI adapter contract passes with Claude Code." This is the second adapter. The architectural surface is identical to `ClaudeCodeCodingAgent`; the only differences are CLI-specific (argv, parser, permission mapper, resume flag).

**Goal:** A `CodexCodingAgent` that implements `CodingAgent`, registered in the `ProviderRegistry` for `kind=cli, binary=<codex>`. Passes the conformance suite from feat-057. Uses the fake CLI harness (extended to emit Codex-style events) for tests.

**Data shape changes:**
- New `CodexCodingAgent` type in `agent::adapters::codex`. Same shape as `ClaudeCodeCodingAgent` (a small struct holding the runner, the parser, the translator, and the permission mapper references).
- New `CodexStreamParser` (sibling of `ClaudeCodeStreamParser`). Reuses the line-stream protocol from feat-043.
- New `CodexPermissionMapper` (sibling of `ClaudeCodePermissionMapper`). Codex-specific permission flags.
- `ProviderRegistry::add_provider` gains a `kind=cli, binary=<codex>` branch that builds a `CodexCodingAgent`.
- The fake CLI's `FAKE_CLI_PERSONALITY=codex` env var switches its event grammar to Codex-style events. This is a small extension to feat-044.

**Implementation outline:**

1. Implement `CodexStreamParser`. The Codex wire format is the implementation detail; the implementer must read Codex's docs during this feature and update this task description with the actual event grammar. The conformance suite's `test_conformance_stream_parser` is the canonical spec.
2. Implement `CodexPermissionMapper`. The mapping is CLI-specific; consult Codex's permission flag docs. The same five `ToolProfile`s map; the CLI flags differ.
3. Implement `CodexCodingAgent::send_message`. The flow is identical to `ClaudeCodeCodingAgent`; the differences are in the adapter-specific config (resume flag, env, CLI flags).
4. Update the `ProviderRegistry` to dispatch on the binary basename (`codex`).
5. Extend the fake CLI with `FAKE_CLI_PERSONALITY=codex` that emits Codex-style events. The fake is already extensible; this is a small new script.
6. Add a `CodexConformanceAdapter` for the conformance suite. Remove the skip for Codex in feat-057.
7. Real-CLI smoke test: a `scripts/smoke_codex.sh` similar to `scripts/smoke_claude_code.sh` from feat-051. Manual only.

**Acceptance criteria:**
- [ ] `test_codex_wrapped_session_create` — create a session with the fake CLI registered as `kind=cli, binary_path=<fake_cli>, FAKE_CLI_PERSONALITY=codex`; assert the session is created with `runtime_kind=claude-code` — wait, NO. `runtime_kind=codex`. (The conformance test for "session create" must reflect the right kind.)
- [ ] `test_codex_wrapped_streams_via_sse` — send a prompt; assert the SSE stream has the expected Codex-style event sequence.
- [ ] `test_codex_resume_cycle` — first turn `resume_state="none"`; second turn `resume_state="native"`. The fake's resume-echo behavior validates the resume id was passed.
- [ ] `test_codex_permission_mapper` — each `ToolProfile` produces a snapshot with the right Codex-specific flags.
- [ ] `cargo test -p weave-server --test cli_conformance` — the Codex adapter passes the full suite. (This is the gate.)
- [ ] `scripts/smoke_codex.sh` exists and is documented.
- [ ] All existing tests pass.

**Verification:**
```bash
cargo test -p weave-server -- test_codex_wrapped_session_create
cargo test -p weave-server -- test_codex_wrapped_streams_via_sse
cargo test -p weave-server -- test_codex_resume_cycle
cargo test -p weave-server -- test_codex_permission_mapper
cargo test -p weave-server --test cli_conformance
./init.sh
```

**Design decisions already made:**
- The architectural surface mirrors `ClaudeCodeCodingAgent`. The conformance suite is the test scaffold; if the adapter passes, it conforms.
- The fake CLI's personality switch (`FAKE_CLI_PERSONALITY=codex`) is a feat-044 extension. The fake is the conformance target; making it multi-personality is simpler than maintaining two fake binaries.

**Design decisions open:**
- Whether to share the `PermissionSnapshot` struct across adapters. Decision: YES (already done in feat-046). Each adapter's mapper produces a `PermissionSnapshot`; the runner consumes it; the struct is uniform.
- Whether Codex's `default_model` is required (vs. shelled out per `list_models`). Same answer as feat-039: required. Cache mitigates the shell-out cost.

**Dependencies:** feat-051, feat-057.

---

### feat-059: `OpenCodeCodingAgent` adapter

**Strategic context:** §3 of the strategy: third CLI adapter. §8: "If a user wants OpenCode pointed at Anthropic, model that as `opencode` with a per-Runtime Tool default model override, not as separate Runtime Tools like `opencode-anthropic`. This keeps the Runtime Tool list short."

**Goal:** A `OpenCodeCodingAgent` that implements `CodingAgent`, registered for `kind=cli, binary=<opencode>`. Passes the conformance suite. The provider's `default_model` is the per-Runtime-Tool model override (no separate `opencode-anthropic` Runtime Tool).

**Data shape changes:** Identical to feat-058 (Codex). New `OpenCodeCodingAgent`, `OpenCodeStreamParser`, `OpenCodePermissionMapper`. The fake CLI gets a `FAKE_CLI_PERSONALITY=opencode` switch.

**Implementation outline:**

1. Implement the OpenCode-specific bits (parser, permission mapper, adapter). Same flow as feat-058.
2. Wire the `default_model` override. The provider row's `default_model` is the model the OpenCode CLI is told to use. The runner adds it to the argv (e.g., `--model <name>` for OpenCode). The `test_opencode_default_model_override_reaches_argv` test asserts this end-to-end.
3. Add a `OpenCodeConformanceAdapter` for the suite. Remove the skip for OpenCode.
4. Real-CLI smoke test: a `scripts/smoke_opencode.sh`. Manual only.

**Acceptance criteria:**
- [ ] `test_opencode_wrapped_session_create` — create a session with the fake CLI registered as `kind=cli, binary_path=<fake_cli>, FAKE_CLI_PERSONALITY=opencode`; assert the session is created with `runtime_kind=opencode`.
- [ ] `test_opencode_wrapped_streams_via_sse` — send a prompt; assert the SSE stream has the expected OpenCode-style event sequence.
- [ ] `test_opencode_resume_cycle` — first turn `resume_state="none"`; second turn `resume_state="native"`.
- [ ] `test_opencode_permission_mapper` — each `ToolProfile` produces a snapshot with the right OpenCode-specific flags.
- [ ] `test_opencode_default_model_override_reaches_argv` — provider row's `default_model=claude-sonnet-4-5`; the runner's argv to the fake includes `--model claude-sonnet-4-5`; the fake echoes it back via a `FAKE_CLI_DUMP_ARGV=1` mode.
- [ ] `cargo test -p weave-server --test cli_conformance` — the OpenCode adapter passes the full suite.
- [ ] `scripts/smoke_opencode.sh` exists and is documented.
- [ ] All existing tests pass.

**Verification:**
```bash
cargo test -p weave-server -- test_opencode_wrapped_session_create
cargo test -p weave-server -- test_opencode_wrapped_streams_via_sse
cargo test -p weave-server -- test_opencode_resume_cycle
cargo test -p weave-server -- test_opencode_permission_mapper
cargo test -p weave-server -- test_opencode_default_model_override_reaches_argv
cargo test -p weave-server --test cli_conformance
./init.sh
```

**Design decisions already made:**
- No separate `opencode-anthropic` Runtime Tool. The provider row's `default_model` is the override. This is the strategy's explicit commitment (§8).
- Same conformance suite as Codex / Claude Code. The conformance is in the adapter's contract, not in the CLI's identity.

**Design decisions open:**
- The exact argv shape for OpenCode. The implementer must read OpenCode's docs during this feature.
- Whether OpenCode's permission model supports the `accept-edits` mode. If not, the `implementation` profile maps to whatever the closest OpenCode equivalent is. Document the mapping in the test.

**Dependencies:** feat-051, feat-057.

---

## Phase 11 — Attended Mode (Deferred)

### feat-060: Attended mode `Terminal` abstraction

**Strategic context:** §3 of the strategy step 4: "Attended terminal mode — deferred until wrapped mode is stable. It is a separate lifecycle and should not block Claude Code wrapped mode." §5: "Attended mode is *not* in the `CodingAgent` trait. It is a different lifecycle (long-lived subprocess, user-driven, not model-driven). It is a separate `Terminal` abstraction that the session page renders, parallel to `CodingAgent`. They share persistence (messages, journey) but not execution." This is the deferred feature.

**Goal:** A separate `Terminal` trait (NOT a `CodingAgent` impl) representing user-driven, long-lived subprocess hosting. The session page's `attended` layout from feat-054 becomes real. The lifecycle: spawn on session start, attach on session page open, detach on page close, kill on session cancel / shutdown.

**Data shape changes:**
- New `Terminal` trait. Distinct from `CodingAgent` (lives in a separate module). Methods: `spawn(session, binary, args, env, cwd) -> TerminalHandle`, `attach(session_id) -> TerminalStream` (a duplex stream for the frontend's xterm.js pane), `detach(session_id)`, `kill(session_id) -> Result`.
- New `TerminalRegistry` (parallel to `ProviderRegistry`). Keyed by session id. Tracks the active `Terminal` for the session.
- New `SessionService::attended_*` methods for the lifecycle. They share persistence (messages, journey) with `send_prompt` but do NOT share execution.
- The fake CLI's `serve` subcommand (an extension to feat-044) emits events as they occur and reads stdin for the user's keystrokes. This is the conformance target for attended mode.
- The `notify` crate is added as a dependency (filesystem watcher for journey feeding).
- Frontend: the `attended` layout from feat-054 is implemented as an xterm.js pane. New dependency on `@xterm/xterm` (or whatever the current xterm.js package is — confirm at implementation time).

**Implementation outline:**

1. Add the `Terminal` trait. The trait is async; the methods return `Future`s. `TerminalStream` is a `tokio::sync::mpsc` of bytes flowing both directions (stdin from the user to the CLI, stdout from the CLI to the xterm.js pane).
2. Add the `TerminalRegistry`. A `HashMap<SessionId, Arc<dyn Terminal>>` behind a `tokio::sync::RwLock`. Built at startup; populated by `Terminal::spawn` and cleared by `kill`.
3. Add `SessionService::attended_spawn(session_id)` — calls the registry's spawn method, which calls the CLI-specific `Terminal` impl (e.g., `ClaudeCodeTerminal`). The CLI is spawned as a long-lived child; the pid is tracked in `ActiveChildProcesses` (feat-049).
4. Add `SessionService::attached_session(session_id) -> TerminalStream` — returns the duplex stream. The session page consumes it.
5. Add `SessionService::attended_kill(session_id)` — sends SIGTERM, waits 5s, SIGKILL. Removes the entry from the registry.
6. Add the filesystem watcher. The `notify` crate watches `cwd` and the matched codebase. File change events are written to the `file_changes` table (reusing the schema from feat-017). This is the "watch" half of "you drive the CLI, Weave watches."
7. Add the CLI's trace event feeding. If the CLI emits structured trace events (e.g., a `WEAVE_TRACE_EVENT=<json>` line on stdout), the terminal impl parses them and writes to the `traces` table. This is opportunistic — not all CLIs emit them; the journey is best-effort in attended mode.
8. Frontend: implement the `attended` layout as an xterm.js pane. The pane is connected to the duplex stream from `attended_session`. User keystrokes go upstream; CLI output renders. Reuse the WebSocket-over-SSE pattern? No — use WebSocket here. Attended mode needs bidirectional streaming; SSE is server-to-client only. Add a `WEBSOCKET_ENABLED` config flag.
9. Tests: the conformance target is the fake CLI's `serve` subcommand. Tests cover spawn, attach, detach, kill, filesystem-watcher-fed journey.

**Acceptance criteria:**
- [ ] `test_terminal_trait_compiles` — the trait compiles; the test is a regression guard.
- [ ] `test_terminal_spawn_and_attach` — `Terminal::spawn` returns a handle; `Terminal::attach` returns a duplex stream; bytes flow in both directions.
- [ ] `test_terminal_detach_keeps_process` — `Terminal::detach` removes the stream from the registry but does NOT kill the process. A subsequent `attach` returns a new stream to the same process.
- [ ] `test_terminal_kill_terminates` — `Terminal::kill` sends SIGTERM, waits, SIGKILL. The process is dead within 6s.
- [ ] `test_terminal_filesystem_watcher_feeds_journey` — the watcher detects a file change in `cwd`; a `file_changes` row is written with the right path and action.
- [ ] `test_terminal_workspaces_scoped_validation` — `Terminal::spawn` with `cwd` outside a registered codebase returns the `cwd_outside_codebase` error from feat-050.
- [ ] `test_terminal_trace_event_parsing` — when the fake CLI emits `WEAVE_TRACE_EVENT=<json>` lines on stdout, the terminal impl writes them to the `traces` table.
- [ ] Frontend: `web/src/app/pages/SessionPage.test.tsx` — the `attended` layout renders an xterm.js pane. The pane connects to the duplex stream.
- [ ] All existing tests pass.

**Verification:**
```bash
cargo test -p weave-server -- test_terminal
cd web && bun run test -- --run terminal-pane
cd web && bun run test
./init.sh
```

**Design decisions already made:**
- WebSocket is the right transport for attended mode (bidirectional). The "SSE is the only real-time transport" hard constraint in CLAUDE.md is relaxed by THIS feature's design; the relaxation is documented as a follow-up CLAUDE.md amendment.
- The `notify` crate is added as a dependency. This is the only new third-party crate introduced by the multi-runtime work; record a `DECISIONS.md` entry.
- Attended mode does NOT re-execute CLI tools. The journey is built from filesystem watching and (where available) CLI trace events. This is best-effort; the user is in charge.

**Design decisions open:**
- The xterm.js frontend package. `@xterm/xterm` is the current home (after the scope reshuffle from `xterm` to `@xterm/xterm`); confirm at implementation time.
- Whether attended mode supports multiple panes (e.g., a side-by-side terminal + chat). Out of scope for v1; defer.
- Whether the filesystem watcher should be workspace-wide or codebase-scoped. Decision: codebase-scoped (matches the cwd scope). Workspace-wide would be a much larger watcher.

**Dependencies:** feat-051.

---

## Cross-cutting documentation

When a Phase 8 / 9 / 10 / 11 feature ships, the following docs MUST be updated in the same PR:

- `docs/api-contracts.md` — any new endpoint, any new SSE event field, any change to the request/response shape. The Phase 9 features change the session create body and the `done` event.
- `docs/provider-abstraction.md` — the `CodingAgent` trait signature change in feat-041.
- `docs/data-model.md` — the schema changes in feat-038, feat-039, feat-055.
- `docs/domain-services.md` — the `SessionService` changes in feat-041, feat-047, feat-049, feat-050.
- `docs/operations.md` — the new env vars (`WEAVE_MODEL_CACHE_TTL_SECS`, `WEAVE_REAP_CLI_PROCESSES_ON_STARTUP`, `WEAVE_A2A_DEFAULT_RUNTIME_KIND`, `WEAVE_DEFAULT_RUNTIME_KIND`) and the WebSocket addition in feat-060.
- `docs/user/sessions.md` — the wrapped-mode UX, the 4-step wizard, the header pills.
- `docs/user/kanban.md` — the column binding.
- `docs/user/providers.md` — the kind-aware form.
- `CHANGELOG.md` — every feature flags its breaking change (the first-provider fallback removal in feat-055/056 is the most user-visible; the new SSE `resume_state` field in feat-047 is the most user-facing).
- `DECISIONS.md` — the `notify` crate addition in feat-060; the WebSocket relaxation in feat-060; the resume id storage location (feat-038, feat-047).

## Risks that should be tracked across phases

These are not blockers but should be revisited as the work progresses:

- **SSE event buffer (100 events per session).** §6 of the strategy: a multi-tool CLI turn may hit the ceiling, after which subscribers see a `gap` event and refetch. Track this once real CLI traces are available in Phase 8 testing. Do NOT pre-emptively grow the buffer.
- **Per-turn subprocess startup cost.** §6: "revisit if a CLI's context-engine costs become visible." A long-lived process preserves in-memory caches. If real CLI traces show the per-turn cost is material, revisit in a future feature.
- **CLI resume id format stability.** Some CLIs may change the resume id format between versions. The persistence in `runtime_metadata_json` is opaque to Weave, so format changes are absorbed — but a stale resume id triggers a replay, which the user sees as a "resume_state=replayed" badge. Track replay counts in `runtime_metadata_json['replay_count']` and surface in the journey if it grows.
- **Real CLI smoke tests.** Every Phase 8 / 10 adapter has a manual smoke test script. These are NOT in CI. If the smoke tests become a regular pre-release ritual, codify them as a separate "smoke" CI job in a future feature.

## Closing notes

This document is the engineering handoff for the strategy in `docs/road-map/multi-runtime-strategy.md`. It does not introduce new decisions; it expands the strategy's commitments into buildable tasks. If a task description here conflicts with the strategy, the strategy wins. If a task description here is unclear, the strategy + the linked code paths (via `ci` tools) are the canonical reference.

Pick the lowest-`not_started` feature whose dependencies are all `passing`. Run `./init.sh` before and after. Don't refactor; don't expand scope; don't touch unrelated code. The WIP=1 rule is enforced.


