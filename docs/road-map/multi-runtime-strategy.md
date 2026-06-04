# Multi-Runtime Strategy

**Status:** proposed · 2026-06-04
**Scope:** the strategic *what* and *why*. The *how* lives in the implementation plan, written later.

## 1. The opportunity

Three local-first coding CLIs — **Claude Code**, **Codex**, **OpenCode** — are now credible primary surfaces for software work. Each is an excellent single-agent experience with a great TUI, its own permission model, and its own session state. None of them gives the user a way to use the other two from the same place, see a single journey across all three, or orchestrate them through a kanban.

Weave is the right home for that conductor layer. It already has:

- a `CodingAgent` trait that abstracts model backends (`docs/provider-abstraction.md`),
- a session/message/trace store that any backend can write through,
- a specialist system that overlays a role-specific system prompt on any backend,
- a kanban that auto-spawns sessions when cards move between columns,
- a journey sidebar that turns trace events into a single human-readable timeline.

Adding Claude Code, Codex, and OpenCode as additional runtimes is a strategic extension, not a new product. This doc records the strategic commitments and the shape of the change. Implementation details (schema migrations, API surface, frontend flow) live in the implementation plan.

## 2. The positioning

> *Weave is the only tool that lets you mix Claude Code, Codex, and OpenCode in the same workflow — and see what all of them did.*

The product sentence: *pick the tool you trust, give it a role, point it at a task, watch every step in one timeline — even when three different tools are running in parallel on the same codebase.*

The developer sentence: *I run Claude Code when I want to think, Codex when I want to ship, and OpenCode when I want to try the new model. Weave is where I keep all of it.*

| What a CLI alone gives | What a CLI alone doesn't give | What Weave adds |
|---|---|---|
| Excellent in-terminal loop | Cross-CLI orchestration | Sessions can be backed by **any** CLI |
| Native tool permissions | Shared state across CLIs | One journey trace, one kanban, one notes/artifact store |
| Its own session state | Multi-workspace / multi-repo context | Workspaces + codebases attach a session to the right repo |
| Its own model picker | A way to compare them on the same task | Side-by-side kanban columns running the same card with different CLIs |
| Its own resume | Resume across machines and CLIs | `parent_session_id` chains sessions across backends |
| Its own TUI | Visible to humans, invisible to other agents | A2A protocol: other agents can call into Weave, and through it into any of the three CLIs |

## 3. The new model: a session has a *runtime* and a *mode*

Today, a session is described by `(provider_id, specialist_id, model, cwd)`. After this change, it grows two orthogonal axes:

```
runtime :  claude-code | codex | opencode | anthropic-api | openai-api | openai-compatible
mode    :  native | wrapped | attended
```

- **`runtime`** — which engine runs the turn. Either a local CLI binary, or an HTTP model API (the current behavior).
- **`mode`** — how the user interacts with the runtime.
  - `native` — Weave calls the model API directly. Today's behavior. Used for HTTP backends.
  - `wrapped` — Weave drives the CLI as a subprocess. One CLI process per turn; the CLI's stream-json becomes Weave's `StreamEvent`. The user sees the same Weave chat surface.
  - `attended` — Weave hosts the CLI in a terminal pane inside the Weave session page. The user types directly into the CLI. Weave watches the filesystem, the git index, and (where the CLI emits them) trace events, and feeds them into the journey sidebar.

The three modes coexist. A kanban column can bind `(runtime, role)`. A card moved into that column auto-spawns a session with that triple. The user can override any of the three at session creation.

## 4. Where the existing model changes

This is the minimal delta from today's model:

- **`Provider` row widens.** Today it is `(type, name, base_url, api_key, default_model)`. After, it is a discriminated union:
  - HTTP: `(name, base_url, api_key, default_model)` — same as today.
  - CLI: `(name, binary, args, env, default_model, permission_mode)` — replaces the HTTP fields.
  `create_agent` becomes a `match` on the runtime kind rather than a hardcoded `"anthropic" | _` arm.
- **`CodingAgent` trait stays the same shape** (`docs/provider-abstraction.md`). The wrapped-CLI implementation is a new `CliCodingAgent` alongside `AnthropicAgent`. The trait was designed for this — *no trait change required*.
- **Specialists stay prompt-only.** A specialist is a role (a system prompt), not a runtime. A specialist rides on top of any runtime. This keeps specialists reusable across CLIs, which is the whole point.
- **Tool filtering splits in two scopes.** In `native` mode, Weave's `ToolRegistry` exposes `fs_*` / `shell_exec` / `git_*` / `task_*` to the model via the API tool-use contract — unchanged. In `wrapped` mode, the CLI runs its own tools; Weave translates the CLI's tool calls into the same `ToolCallBlock` for the UI and the same `TraceCollector` events for the journey, but does *not* re-execute them. The `ToolProfile` (full / implementation / review / planning / reporting) maps to the CLI's `--allowedTools` / `--disallowedTools` flags.
- **A session grows a CLI-native session id column** per supported CLI. When Weave restarts, it can resume a CLI subprocess by the CLI's own id (Claude Code's `--resume <id>`, Codex's `codex exec resume <thread>`, OpenCode's `--session <id>`). This is what makes resume robust.
- **Attended mode is *not* in the `CodingAgent` trait.** It is a different lifecycle (long-lived subprocess, user-driven, not model-driven). It is a separate `Terminal` abstraction that the session page renders, parallel to `CodingAgent`. They share persistence (messages, journey) but not execution.
- **A2A already works.** External agents can already call Weave's A2A endpoints (`docs/...`); once sessions are runtime-agnostic, an external agent calling into Weave can pick which runtime runs its request.

## 5. The non-obvious calls

These are the choices I want made *before* the implementation plan:

- **Wrapped and attended are different things, not two flavors of the same thing.** Wrapped is "Weave drives the CLI." Attended is "You drive the CLI, Weave watches." Mixing them would make cancel / resume / kanban auto-trigger confusing. Keep them as separate `mode` values with separate code paths.
- **The CLI's own session id is the durable key for resume, not the Weave session id.** Store the CLI id on the session row; pass it to the CLI on `send_prompt` when present. When the CLI is gone or the user changes runtime, fall back to re-sending the Weave message history as a fresh prompt.
- **Permission mode is per-tool, not per-session.** Different CLIs handle permissions differently; the tool config owns that decision. The session header shows the active permission mode so the user always sees it.
- **Models come from the tool, not from Weave.** Claude Code knows its own model list. Weave should not duplicate that knowledge. `list_models()` on `CliCodingAgent` shells out to the CLI (or reads from a cached `claude models` / `codex models` / `opencode models` output refreshed on health check).
- **The journey sidebar is the unifying artifact.** It already records `Decision`, `FileChange`, `Error`, `Milestone`, `Review`. The wrapped-mode translator extracts Decisions from each CLI's thinking blocks and FileChanges from `Bash` / `Read` / `Write` / `Edit` invocations, so the journey looks the same regardless of which CLI produced it. That is the single thing that makes "see what all of them did" real.
- **Per-turn subprocess, not long-lived, for `wrapped` mode.** Spawn the CLI per `send_message` with the resume flag. Long-lived preserves in-memory caches but couples Weave's session lifecycle to the CLI's process lifecycle. Start simple; revisit if a CLI's context-engine costs become visible.
- **The `Multiple concurrent providers` drop in `docs/SYSTEM_DESIGN.md` is amended.** That drop recorded the v1 limit of "one active provider per session." Multi-runtime replaces it: *one active runtime per session, freely swappable between Anthropic API, OpenAI API, and any registered CLI; multi-runtime *inside* a session is not a goal.*

## 6. The interaction shape (user-facing)

The interaction model has four steps in this fixed order, both for human-driven and kanban-driven session creation:

1. **Tool** — which engine runs the session (Claude Code / Codex / OpenCode / Anthropic API / OpenAI API / OpenAI-compatible).
2. **Role** — which system prompt the agent runs under (specialist).
3. **Model** — which model within the tool.
4. **What it works on** — workspace, codebase, optional kanban task.

This replaces the current three-text-field "New Session" modal. The Settings page replaces its hardcoded `type: "anthropic"` with a tool-aware form. Kanban columns can bind `(runtime, role)` so the same card can flow through three different CLIs on three different boards. The session page itself renders one of three layouts (`native` / `wrapped` / `attended`) from the same `/session/:id` URL.

These interactions are *what* the strategic plan commits to. The exact field shapes, the API endpoints, and the React components are the implementation plan's job.

## 7. Open questions to resolve before implementation

- **Hosted vs. self-hosted CLI models.** If a user wants OpenCode pointed at Anthropic, do we model that as one tool (`opencode-anthropic`) or as `opencode` with a per-tool default model override? Recommended: the override. Keeps the tool list short.
- **Resume fallback.** When the CLI for a session is gone, do we (a) refuse to resume, (b) re-send the Weave message history as a fresh prompt to whichever runtime the user picks at resume time, or (c) refuse for the original runtime but allow resume under a different one? Recommended: (b) always, (c) optionally.
- **Cost attribution.** Each CLI has its own billing model. Weave can show token counts from the `step_finish.usage` / `message_delta.usage` events; the dollar cost is the user's problem. Surface tokens prominently; don't pretend to do billing.
- **A2A exposure of wrapped sessions.** Should external A2A callers be allowed to pick a runtime per request, or should they get whatever runtime Weave last used for the target session? Recommended: pick a runtime per request. It matches the "conductor, not instrument" framing.

## 8. What this doc is not

This is the *what* and the *why*. It is not:

- A schema migration. The implementation plan will add the `runtime` and `mode` columns and the per-CLI session-id columns.
- A trait redesign. The trait is the right shape; the implementation plan adds `CliCodingAgent` and `Terminal` as new impls.
- A frontend plan. The implementation plan will sketch the four-step session creation sheet and the three session layouts.
- A compatibility promise. The exact list of supported CLIs and their permission modes is a release-time decision.

The implementation plan will turn each section of this doc into scoped features (`feature_list.json` entries with behavior, verification, and dependencies) and then into file-by-file tasks.
