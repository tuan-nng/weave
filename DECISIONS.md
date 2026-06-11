# DECISIONS.md

<!--
Non-obvious architectural and design choices, with the reason.
Records the "why" that gets lost when context is compacted or sessions reset.
NOT a code-review log. Only decisions that would otherwise force re-derivation.

Format: dated, scoped, with rejected alternatives.
Newest at the top.
-->

## 2026-06-11: feat-047 — `ResumeState` lives in `agent::mod` next to `RuntimeKind`/`SessionMode`

- **Decision:** The new `ResumeState` enum (3 variants: `None`/`Native`/`Replayed`) is added to `crates/weave-server/src/agent/mod.rs` next to the existing wire enums `RuntimeKind` and `SessionMode`. Same shape: `Default` (returns `None`), `as_str` (snake_case wire form), `Display`, `#[serde(rename_all = "snake_case")]`. A new `pub(crate) fn decide(had_resume_attempt: bool, did_reject: bool, should_persist_capture: bool) -> Self` method holds the 4-arm state-machine match; a truth-table test covers all 8 input combinations.
- **Reason:** Consistent with the project's "wire enums live in `agent::mod`" pattern. The `SseWireEvent::Done` and `SseWireEvent::MessagePersisted` variants already reference `agent::RuntimeKind` and `agent::SessionMode`; a third wire enum in the same module is the obvious home. The `decide` method is the right shape for unit tests (8-case truth table in 25 lines) and for future state-machine extensions (e.g., when a second CLI runtime adds a new variant or a new arm). `pub(crate)` is the correct tier: callers are inside the crate (`service::sessions::run_prompt_task` and the conformance suite in feat-057), but external consumers (frontend, conformance suite) only need the enum and `as_str`, not the method.
- **Rejected alternatives:**
  - Sibling module `agent::resume_state` — deferred to feat-051 when a second consumer (the `ClaudeCodeCodingAgent`'s `cli_rejection` injection site) lands. The current single-call-site is below the abstraction bar; a sibling module is the right move when there are 2+ methods that share the module's purpose.
  - Method on `LoopResult` (e.g., `LoopResult::resume_state() -> ResumeState`) — couples the state-machine computation to the result struct, which mixes "facts a turn exposes" (the inputs to `decide`) with "facts about the loop's success" (`stop_reason`, `cancelled`). The current shape keeps `decide` as a pure function of three booleans; the call site in `run_prompt_task` assembles the booleans from `turn_ctx` + `loop_result` + `stop_reason`, keeping each layer's responsibility crisp.
  - `String` field on `SseWireEvent::Done` instead of a typed enum — would let the frontend (feat-054) and conformance suite (feat-057) branch on free-form strings. The enum's `as_str` is the canonical source of the wire form; a `String` would diverge over time.
- **Constraints introduced:** `ResumeState` lacks a `FromStr` impl. The current callers only serialize the value to the SSE wire (via `serde_json::to_string` + the enum's `#[serde(rename_all)]`); no string-input consumer exists. When the conformance suite (feat-057) or the frontend (feat-054) needs to parse a stored `resume_state`, add the `FromStr` impl mirroring `SessionMode::from_str` (with `AppError::validation` on unknown). Defer to the first consumer per the "no premature abstraction" rule.
- **Revisit when:** A second CLI runtime (Codex in feat-058, OpenCode in feat-059) needs to populate the `did_reject` input to `decide`. At that point, decide whether to widen `decide` to take the runner outputs directly (e.g., `decide_for_turn(&session, &turn_ctx, &LoopResult)`) or to add a `LoopResult::cli_rejection: Option<bool>` field that the call site passes through. The current signature is already shaped for either path.

## 2026-06-04: Multi-runtime strategy committed

- **Decision:** Adopt the multi-runtime direction recorded in [`docs/road-map/multi-runtime-strategy.md`](docs/road-map/multi-runtime-strategy.md), with the **native Anthropic tool-execution loop as the first prerequisite** and **Claude Code CLI wrapped mode as the first CLI implementation target**. Sessions grow `runtime` and `mode` columns. The `Provider` table widens to a discriminated union (HTTP vs CLI). `CliCodingAgent` is added alongside `AnthropicAgent`, but the exact request/context shape must be revisited before the first CLI adapter lands. Codex and OpenCode follow after the shared adapter contract is proven. Attended mode is a separate `Terminal` abstraction, parallel to `CodingAgent`.
- **Reason:** Claude Code, Codex, and OpenCode are now credible primary coding surfaces. None of them gives the user a conductor layer. Weave already has the trait shape, the journey/trace store, and the kanban — adding the three CLIs is a strategic extension, not a new product.
- **Rejected alternatives:**
  - Stay single-runtime (Anthropic API only) — rejected: leaves Weave as one of many single-model tools, with no compelling reason to exist once a user has Claude Code installed.
  - Add WebSocket / process-spawning into the HTTP path — rejected: violates the SSE-only transport decision and conflates the HTTP agent model with the subprocess model.
  - Implement multi-runtime by spawning a local HTTP proxy per CLI — rejected: adds an unnecessary process and a wire-format conversion the OS can do for us.
  - Add "attended mode" as a `CodingAgent` impl — rejected: attended mode is user-driven, not model-driven. A single trait cannot represent both lifecycles cleanly. Kept separate.
- **Constraints introduced:** A session table migration is required to add `runtime` and `mode` plus CLI-native session-id metadata. Existing rows default to `runtime = "anthropic-api"`, `mode = "native"`. The `Provider` table migration is additive (new config fields, no rename of existing ones). The first implementation plan starts with native Anthropic tool-loop completion, then adds the fake CLI harness and Claude Code wrapped-mode adapter before Codex/OpenCode work begins.
- **Revisit when:** The implementation plan is written and before the first CLI adapter lands — verify whether `MessageRequest`, `CodingAgent`, or a separate runtime-turn context should carry cwd/codebase, resume metadata, effective permissions, and process lifecycle hooks.

## 2026-05-31: Single Rust binary with embedded frontend

- **Decision:** Build a single Rust binary that embeds the compiled React frontend at build time via `build.rs`. No separate Node.js runtime in production.
- **Reason:** Simplifies deployment to a single executable. Eliminates CORS, reverse proxy config, and Node.js runtime dependency. Linux-first target.
- **Rejected alternatives:**
  - Separate frontend server (Vite/Nginx) — rejected: adds operational complexity, CORS config, two processes to manage
  - Serve static files from filesystem at runtime — rejected: single binary is cleaner, no path configuration needed
- **Constraints introduced:** `build.rs` must run frontend build before Rust compilation. Dev mode uses Vite dev server with proxy to backend.
- **Revisit when:** Frontend build time exceeds 30s and blocks iteration speed.

## 2026-05-31: SQLite with WAL mode, no ORM

- **Decision:** Use `rusqlite` directly with WAL mode. No ORM (Diesel, SeaORM, etc.).
- **Reason:** Single-user/small-team tool. SQLite is embedded, zero-config, and WAL handles concurrent reads well. Raw SQL gives full control over the 11-table schema without ORM abstraction overhead.
- **Rejected alternatives:**
  - Diesel — rejected: macro-heavy, compile-time schema doesn't match rapid iteration phase
  - SeaORM — rejected: async wrapper around SQLite adds complexity for minimal gain
  - Postgres — rejected: requires separate process, overkill for workspace-scoped tool
- **Constraints introduced:** All queries must be parameterized (no string interpolation). Migrations are manual SQL files in `resources/migrations/`.
- **Revisit when:** Multi-user concurrent write workload exceeds SQLite's WAL capacity (~100 concurrent writers).

## 2026-05-31: SSE for all real-time communication

- **Decision:** Server-Sent Events (SSE) as the sole real-time transport. No WebSocket.
- **Reason:** SSE is simpler, works over standard HTTP, auto-reconnects, and is sufficient for server-to-client streaming (agent responses, trace events, kanban updates). Client-to-server communication uses regular HTTP POST.
- **Rejected alternatives:**
  - WebSocket — rejected: bidirectional not needed, adds protocol complexity, harder to proxy/load-balance
  - Long polling — rejected: higher latency, more resource waste
- **Constraints introduced:** All streaming endpoints follow `/api/{resource}/stream` pattern. Event buffer must handle client disconnect/reconnect with `Last-Event-ID`.
- **Revisit when:** True bidirectional streaming is needed (unlikely for this architecture).

## 2026-05-31: Provider abstraction via CodingAgent trait

- **Decision:** Define a `CodingAgent` trait that abstracts provider capabilities. Anthropic is the first implementation.
- **Reason:** Enables adding OpenAI, local models, or custom providers without changing session/kanban logic. Trait-based design is idiomatic Rust.
- **Rejected alternatives:**
  - Hardcode Anthropic — rejected: limits extensibility, contradicts platform positioning
  - Plugin system with dynamic loading — rejected: premature complexity for v1
- **Constraints introduced:** `StreamEvent` enum is the universal streaming contract. Providers must implement `send_message`, `list_models`, `health_check`.
- **Revisit when:** Second provider is added — validate the trait shape still fits.

## 2026-06-04: Split SYSTEM_DESIGN.md into per-topic docs

- **Decision:** Split the 2,083-line monolithic `docs/SYSTEM_DESIGN.md` into 10 focused topic docs (average 150 lines each) plus a 127-line routing map. Delete §3 (Module Design) — now covered by 14 `_INDEX.md` files co-located with source.
- **Reason:** A 2,083-line monolithic doc suffers from "lost in the middle" (§§8-15 were in the weakest attention zone), mixed granularity (reference material next to design rationale), and duplication with per-module `_INDEX.md` files. The split follows harness engineering principle §4: each topic doc is 50-150 lines, loaded only when relevant.
- **Rejected alternatives:**
  - Keep as-is — rejected: 2,083 lines = 14x the recommended topic doc size, middle sections ignored, guaranteed divergence from co-located `_INDEX.md` files
  - Add a table of contents — rejected: doesn't fix lost-in-the-middle, still a context bomb when loaded
  - Split into fewer docs — rejected: provider abstraction at 425 lines alone needed its own doc
- **Constraints introduced:** Topic docs must stay focused on their domain. New system-level design content goes into the appropriate topic doc, not appended to SYSTEM_DESIGN.md. CLAUDE.md lists all topic docs with applicability conditions (when to load each).
- **Revisit when:** SYSTEM_DESIGN.md routing map exceeds 200 lines or any topic doc exceeds 250 lines — split further or audit for overlap.

## 2026-05-31: Workspace-scoped resources

- **Decision:** Every resource (sessions, boards, providers) belongs to a workspace. Default workspace created on first startup.
- **Reason:** Multi-workspace support is a core design goal. Scoping from the start prevents painful migration later.
- **Rejected alternatives:**
  - Global resources with optional workspace — rejected: leads to inconsistent scoping, "orphan" resources
  - Multi-tenant with tenant_id — rejected: workspaces are the tenant concept, no need for a separate layer
- **Constraints introduced:** Every DB query must include `workspace_id`. API routes are nested under `/api/workspaces/:wid/`.
- **Revisit when:** Never — this is a permanent architectural choice.
