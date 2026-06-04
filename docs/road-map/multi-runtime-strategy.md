# Multi-Runtime Strategy

**Status:** committed strategic direction · 2026-06-04
**Scope:** development direction and implementation sequencing. Claude Code CLI is the first wrapped CLI target; Codex and OpenCode follow only after the shared CLI adapter is proven.

Terminology in this document is intentional:

- **Runtime Tool** means the user-selectable engine that runs a session.
- **Weave tool** means an executable capability exposed through `ToolRegistry`, `ToolExecutor`, and `ToolProfile`.
- The current Rust, database, and API implementation may continue to call Runtime Tools **Provider** until a deliberate migration changes those names.

## 1. The opportunity

Three local-first coding CLIs — **Claude Code**, **Codex**, **OpenCode** — are now credible primary surfaces for software work. Each is an excellent single-agent experience with a great TUI, its own permission model, and its own session state. None of them gives the user a way to coordinate CLI-backed sessions through Weave's kanban, trace, notes, artifacts, and A2A surfaces.

The implementation deliberately prioritizes **Claude Code CLI first**. Claude Code is the proving runtime for wrapped CLI execution, stream normalization, permission mapping, CLI-native resume, and journey capture. Codex and OpenCode are expansion targets after the shared adapter contract is tested against Claude Code in production-like flows.

Weave is the right home for that conductor layer. It already has:

- a `CodingAgent` trait that abstracts execution backends (`docs/provider-abstraction.md`),
- an internal Provider registry/store that can be widened from HTTP-only config to HTTP-or-CLI config,
- a session/message/trace store that any backend can write through,
- a specialist system that overlays a role-specific system prompt on any backend,
- a kanban that auto-spawns sessions when cards move between columns,
- a journey sidebar that turns trace events into a single human-readable timeline.

Adding CLI-backed Runtime Tools is a strategic extension, not a new product, but it should be built incrementally. This doc records the strategic commitments and the development order. Implementation details (schema migrations, API surface, frontend flow) live in the implementation plan.

## 2. The development objective

The first deliverable is not a general-purpose multi-runtime matrix. It is a safe, tested path for running **Claude Code CLI** through the existing Weave session lifecycle, after the native Anthropic tool loop is made honest.

| Development need | Roadmap commitment |
|---|---|
| Preserve current API provider behavior | Anthropic API/native sessions stay green throughout the work |
| Complete native tool execution | Native mode runs Weave tools and resumes the model turn correctly |
| Add CLI subprocess execution | Claude Code runs through a `CliCodingAgent` in wrapped mode |
| Normalize runtime output | CLI output maps into the existing `StreamEvent` contract |
| Keep journey useful | API and CLI tool activity becomes trace/journey evidence |
| Support resume | Weave stores each CLI's native resume metadata |
| Keep permissions explicit | Weave tool profiles map to the effective permission snapshot for the selected Runtime Tool |
| Make later runtimes cheaper | Codex/OpenCode reuse the tested CLI runner, parser framework, fake CLI harness, and conformance tests |

## 3. Development priority: native tool loop, then Claude Code CLI

The runtime rollout is intentionally sequential:

0. **Prerequisite: complete the native-mode tool-execution loop** — `run_prompt_task` currently sends tool definitions to the provider, but it does not complete the API tool-use loop. The prerequisite feature must bring native Anthropic mode into conformance with the execution model in `docs/provider-abstraction.md`: collect streamed tool input, execute allowed Weave tools through `ToolRegistry` / `ToolExecutor`, append provider-native tool-result blocks, and continue model calls while the provider stops for tool use. The implementation plan should cover cancellation, malformed input, unknown tools, tool errors, and loop limits, but the strategic point is simpler: native mode must run the tools it advertises before wrapped CLIs become the proving ground for journey capture.
1. **Claude Code CLI in `wrapped` mode** — first multi-runtime implementation target. Build the CLI subprocess harness, fake CLI test fixture, stream parser, permission mapper, runtime session-id capture, resume behavior, cancellation behavior, and journey translation around Claude Code.
2. **Anthropic API in `native` mode remains green** — every step must preserve the Anthropic provider path and existing session/SSE/message persistence behavior. This guarantee becomes meaningful once step 0 has made native tool execution real, not text-only.
3. **Codex and OpenCode adapters** — added only after the shared CLI adapter contract passes with Claude Code. They should reuse the subprocess runner, parser framework, permission snapshot model, fake CLI harness, and adapter conformance tests.
4. **Attended terminal mode** — deferred until wrapped mode is stable. It is a separate lifecycle and should not block Claude Code wrapped mode.

This order avoids designing for three different CLIs before one concrete CLI has exercised the abstraction, and it avoids designing a CLI adapter on top of an incomplete native tool loop. The first implementation plan should therefore lead with the native tool-execution loop feature, then create feature-list entries around Claude Code wrapped mode, not around generic multi-runtime support.

## 4. The new model: a session has a *Runtime Tool* and a *mode*

Today, a session is described by `(provider_id, specialist_id, model, cwd)`. After this change, it grows two user-visible axes, backed by internal Provider/runtime configuration:

```
tool / runtime kind :  claude-code | codex | opencode | anthropic-api | openai-api | openai-compatible
mode                :  native | wrapped | attended
```

- **`tool`** — user-facing label for the engine that runs the session. In code, this may still resolve to a Provider row, Provider ID, and ProviderRegistry entry.
- **`runtime kind`** — implementation family behind a Runtime Tool, either a local CLI binary or an HTTP model API.
- **`mode`** — how the user interacts with the Runtime Tool.
  - `native` — Weave calls the model API directly. Today's behavior. Used for HTTP backends.
  - `wrapped` — Weave drives the CLI as a subprocess. One CLI process per turn; the CLI's stream-json becomes Weave's `StreamEvent`. The user sees the same Weave chat surface.
  - `attended` — Weave hosts the CLI in a terminal pane inside the Weave session page. The user types directly into the CLI. Weave watches the filesystem, the git index, and (where the CLI emits them) trace events, and feeds them into the journey sidebar.

The three modes coexist. A kanban column can bind `(tool, role)` and, later, an optional model override. A card moved into that column auto-spawns a session with that binding. The user can override Runtime Tool, role, model, and target work at session creation.

### Runtime Tool × mode compatibility

Not every `(tool, mode)` pair is meaningful. The plan commits to this matrix:

| Runtime Tool / runtime kind \ mode | `native` | `wrapped` | `attended` |
|---|---|---|---|
| `anthropic-api` | ✅ supported (current) | ❌ rejected at session creation | ❌ rejected at session creation |
| `openai-api` | ✅ supported (future) | ❌ rejected at session creation | ❌ rejected at session creation |
| `openai-compatible` | ✅ supported (future) | ❌ rejected at session creation | ❌ rejected at session creation |
| `claude-code` | ❌ rejected at session creation | ✅ supported (first target) | 🟡 deferred |
| `codex` | ❌ rejected at session creation | 🟡 after Claude Code | 🟡 deferred |
| `opencode` | ❌ rejected at session creation | 🟡 after Claude Code | 🟡 deferred |

The rule: HTTP runtime kinds serve `native` only; CLI runtime kinds serve `wrapped` (and later `attended`). Mixing them is rejected at session creation with a clear error. This avoids a class of bugs where a `native` session on a CLI Runtime Tool tries to call an HTTP endpoint that doesn't exist, or a `wrapped` session on an HTTP Provider tries to spawn a subprocess that doesn't exist.

## 5. Where the existing model changes

This is the minimal delta from today's model:

- **The internal Provider config becomes discriminated HTTP vs CLI config; the UI presents each registered row as a Runtime Tool.** Today Provider creation, registry construction, and the Settings form are Anthropic-shaped. They need to move to a kind-aware config model:
  - HTTP Provider: `(name, base_url, api_key, default_model)` — same as today.
  - CLI Provider: `(name, binary, args, env, default_model, permission_mode)` — replaces the HTTP fields.
  The first implementation does not need to rename `/api/providers` or Rust `Provider*` types if doing so would create churn before the CLI adapter is proven.
- **`CodingAgent` trait shape must be revisited before the first CLI adapter lands** (`docs/provider-abstraction.md`). The existing stream contract is the right starting point, and the wrapped-CLI implementation should still be a `CliCodingAgent` alongside `AnthropicAgent`. But CLI-backed turns need per-session execution context — cwd/codebase, runtime metadata for CLI-native resume, effective permissions, and process lifecycle hooks — so the implementation plan must decide whether to extend `MessageRequest` or introduce a separate runtime-turn context.
- **Specialists stay prompt-only.** A specialist is a role (a system prompt), not a Runtime Tool. A specialist rides on top of any Runtime Tool. This keeps specialists reusable across CLIs, which is the whole point.
- **Weave-tool execution and Runtime Tool permissions split by mode.** In `native` mode, Weave owns executable-tool calls through `ToolRegistry` / `ToolExecutor` and replays tool results through the provider API. In `wrapped` mode, the CLI runs its own tools; Weave translates the CLI's tool calls into the same UI and trace shapes, but does *not* re-execute them. The `ToolProfile` (full / implementation / review / planning / reporting) maps to each CLI's effective permission model via a per-provider-kind `PermissionMapper` (see §6).
- **Sessions need persistent runtime/mode and CLI resume metadata.** Existing sessions backfill to Anthropic API native mode. CLI-specific state, including the CLI's native session id, lives in generic runtime metadata rather than one schema column per CLI.
- **Attended mode is *not* in the `CodingAgent` trait.** It is a different lifecycle (long-lived subprocess, user-driven, not model-driven). It is a separate `Terminal` abstraction that the session page renders, parallel to `CodingAgent`. They share persistence (messages, journey) but not execution.
- **A2A and kanban must stop silently choosing the first Provider once Runtime Tools are explicit.** Multi-runtime implementation must add explicit Tool/provider selection for new A2A requests and kanban column bindings. See §8 for the fallback policy; "first provider in the list" is not a valid multi-runtime policy.

## 6. The non-obvious calls

These are the choices to carry into the implementation plan:

- **Wrapped and attended are different things, not two flavors of the same thing.** Wrapped is "Weave drives the CLI." Attended is "You drive the CLI, Weave watches." Mixing them would make cancel / resume / kanban auto-trigger confusing. Keep them as separate `mode` values with separate code paths.
- **The CLI's own session id is the durable key for resume, not the Weave session id.** Store the CLI id in runtime metadata and pass it to the CLI on the next turn when present. If the CLI rejects it or the user switched Runtime Tools, fall back to message-history replay. Surface the chosen path in the session header so the user knows which mode they're in.
- **Permission mode is per Runtime Tool, and per provider/runtime kind.** Different CLIs handle permissions differently, and a `ToolProfile` cannot map to one universal flag set. The implementation plan must define a `PermissionMapper` contract and concrete effective-permission snapshots, starting with Claude Code and stubbing later CLIs only as far as the CLI adapter contract requires.
- **Models come from the Runtime Tool, not from Weave, and `list_models` needs its own cache.** CLI model discovery shells out to the selected Runtime Tool and is slower than the existing provider health check. The implementation plan should add a longer-lived model cache keyed on Provider/Tool id, with explicit refresh and invalidation behavior. The existing 10s health-check TTL stays as-is.
- **The journey sidebar is the unifying artifact, for both fixed native mode and wrapped mode.** Native mode becomes honest once §3 step 0 records tool execution and file changes through Weave's `ToolExecutor` path. Wrapped mode translates each CLI's thinking/tool/file-change stream into the same trace shapes, without re-executing CLI tools. That is the single thing that makes "see what all of them did" real.
- **Per-turn subprocess, not long-lived, for `wrapped` mode.** Spawn the CLI per `send_message` with the resume flag. Long-lived preserves in-memory caches but couples Weave's session lifecycle to the CLI's process lifecycle. Start simple; revisit if a CLI's context-engine costs become visible. Cancellation and startup cleanup must account for child processes, not just database session rows.
- **SSE event buffer size is a tracked risk, not a blocker.** The buffer is 100 events per session (`docs/sse-design.md`). A multi-tool CLI turn may hit the ceiling, after which subscribers see a `gap` event and refetch. Track as a known issue to revisit once real CLI traces are available; do not pre-emptively grow the buffer.
- **The `Multiple concurrent providers` drop in `docs/SYSTEM_DESIGN.md` is amended.** That drop recorded the v1 limit of "one active provider per session." Multi-runtime replaces it: *one active Runtime Tool/internal Provider per session, freely swappable between Anthropic API, OpenAI API, and any registered CLI; multi-runtime inside a session is not a goal.* See §8 for A2A fallback policy.

## 7. The interaction shape (user-facing)

The interaction model has four steps in this fixed order, both for human-driven and kanban-driven session creation:

1. **Runtime Tool** — which engine runs the session (Claude Code / Codex / OpenCode / Anthropic API / OpenAI API / OpenAI-compatible).
2. **Role** — which system prompt the agent runs under (specialist).
3. **Model** — which model within the Runtime Tool.
4. **What it works on** — workspace, registered codebase, optional kanban task.

This replaces the current three-text-field "New Session" modal. The Settings page replaces its hardcoded `type: "anthropic"` with a Runtime Tool-aware form, even if the backend endpoints remain named `/api/providers` during the first implementation. Kanban columns can bind `(tool, role)` so the same card can flow through different CLIs or API backends on different boards. The session page itself renders one of three layouts (`native` / `wrapped` / `attended`) from the same `/session/:id` URL.

These interactions are *what* the strategic plan commits to. The exact field shapes, the API endpoints, and the React components are the implementation plan's job.

## 8. Resolved policy decisions for the implementation plan

- **Hosted vs. self-hosted CLI models.** If a user wants OpenCode pointed at Anthropic, model that as `opencode` with a per-Runtime Tool default model override, not as separate Runtime Tools like `opencode-anthropic`. This keeps the Runtime Tool list short.
- **Resume fallback.** Use the policy in §6: CLI-native session id first, message-history replay when resume fails or the user switches Runtime Tools.
- **Cost attribution.** Each CLI has its own billing model. Weave can show token counts from provider/CLI usage events; the dollar cost is the user's problem. Surface tokens prominently; don't pretend to do billing.
- **A2A exposure of wrapped sessions.** External A2A callers may specify a Runtime Tool per request when the user has permitted it. Existing A2A callers that omit it inherit the session's current binding or an explicit configured default. The current first-provider fallback must be replaced before multi-runtime A2A is considered complete.
- **Workspace-scoped CLI sessions.** Wrapped CLI sessions must run inside a registered `Codebase` row. The session creation modal should reject wrapped sessions whose `cwd` is outside a registered codebase because kanban auto-spawn and trace storage both rely on workspace-scoped codebase context.
- **Child-process reaping on crash.** Normal cancel kills the tracked child process. Startup also scans for surviving CLI processes associated with inactive sessions; database-only orphan reaping is not enough once wrapped CLIs exist.

## 9. What this doc is not

This is the development direction and sequencing. It is not:

- An implementation plan. This doc makes the strategic calls (prerequisite, sequencing, Runtime Tool × mode matrix, schema direction, non-obvious decisions, resolved policies). The implementation plan is the next deliverable and will turn each section into scoped `feature_list.json` entries with behavior, verification, and dependencies. The first entry is the native tool-execution loop, not a CLI adapter.
- An endpoint rename mandate. The UI should say Runtime Tool, but existing Rust/API/DB names that say Provider can stay until an implementation step deliberately migrates them.
- A final trait redesign. The existing stream contract is the right starting point, but the implementation plan must decide how CLI turn context reaches `CliCodingAgent`.
- A frontend plan. The implementation plan will sketch the four-step session creation sheet and the three session layouts.
- A compatibility promise. The exact list of supported CLIs and their permission modes is a release-time decision.
