# Kanban × Claude Code Agent Coordination

**Status:** design proposal
**Scope:** how the Claude Code CLI agent interacts with kanban boards — prompt construction, tool surface, lifecycle management, and gate enforcement
**Depends on:** feat-051 (Claude Code wrapped sessions), feat-057 (shared CLI conformance)
**Reference implementation:** Routa `src/core/kanban/` (8,784 lines TypeScript)

---

## 1. Problem

Weave's kanban automation fires a Claude Code agent when a card moves to an auto-trigger column, but the agent receives almost no kanban context and has no way to talk back to the board. The current flow is:

```
Card moves → try_automate_lane → create session → send "Process task: {title}\n{description}" → agent runs → ???
```

The agent cannot see which column it is in, what gates block the next move, what artifacts are required, or what the previous lane did. It has no tools to update the card, move it forward, or provide evidence. When the agent stalls or fails, the card stays stuck forever with no recovery.

This is a fire-and-forget design. The kanban is a trigger, not a coordination surface.

## 2. Goals

1. **Rich prompt** — the agent receives structured kanban context (column, gates, history, artifacts, story readiness) in its initial prompt.
2. **Bidirectional tools** — the agent can call `move_card`, `update_card`, `update_task`, `list_artifacts`, `provide_artifact` to interact with the board.
3. **Column-stage awareness** — the prompt, available tools, and instructions differ by column stage (backlog, todo, dev, review, done).
4. **Gate enforcement** — card moves are validated against artifact, delivery, and contract gates before execution.
5. **Lifecycle management** — stalled or failed sessions are detected and recoverable.

## 3. Non-goals

- Multi-step lane automation (a column running multiple specialist steps sequentially) — deferred until single-step coordination is proven.
- Flow diagnosis and reasoning memory — lower priority, add after basic coordination works.
- Attended mode kanban integration — separate lifecycle.
- A2A-triggered kanban sessions — existing A2A path is unchanged.

## 4. Current State

### Weave (today)

| Component | Implementation | Lines |
|---|---|---|
| Lane automation | `service/kanban.rs` `try_automate_lane` | 917 |
| Initial prompt | `"Process task: {title}\n{description}"` | 3 lines |
| Specialist format | Flat `.md` files with frontmatter | 5 files |
| Transition gates | `tools/kanban/move_card.rs` `check_transition_gates` | minimal |
| Agent tools | `fs_read`, `fs_write`, `fs_edit`, `fs_search`, `fs_list`, `shell_exec` | no kanban tools |
| Lifecycle | None — fire-and-forget | 0 |

### Routa (reference)

| Component | Implementation | Lines |
|---|---|---|
| Lane automation | `workflow-orchestrator.ts` + `agent-trigger.ts` | 959 + 904 |
| Initial prompt | `buildTaskPrompt()` with 12+ sections | ~580 lines |
| Specialist format | YAML with `id`, `role`, `model_tier`, `default_adapter` | 10+ files |
| Transition gates | `transition-gates.ts` (artifact, delivery, contract, checklist, validator, human approval) | 148 |
| Agent tools | Per-column MCP tool list with card-specific IDs | ~20 tools |
| Lifecycle | Watchdog (30s scan), watchdog_retry, ralph_loop recovery | 300+ |

## 5. Design

### 5.1 Rich Prompt Construction

Replace `build_initial_prompt` with a structured prompt builder that assembles context from the task, column, board, and session state.

**Prompt sections (in order):**

```
You are assigned to Kanban task: {title}

## Context
You are working in Kanban context. Use tools to manage this card.

## Task Details
Card ID, Board ID, Current Column, Next Column, Priority, Labels, GitHub Issue

## Objective
{task.objective or task.description}

## Story Readiness (if backlog column)
Scope, acceptance criteria, verification commands, test cases — what's present, what's missing

## Artifact Gates
Current lane requires: {list of required artifact types}
Next transition requires: {list of required artifact types}
Use list_artifacts to confirm, provide_artifact to fill gaps.

## Delivery Gates (if next column has delivery rules)
Required: committed changes, clean worktree, PR-ready branch

## Contract Gates (if next column requires canonical story)
Required: valid canonical YAML story block in description

## Lane History
Previous run in this lane: {provider, role, start/end time, status}

## Lane Handoff Context
Previous lane session: {descriptor}
Pending handoffs: {list}

## Available Tools
Column-specific tool list with card-specific IDs

## Instructions
Column-stage-specific step-by-step guidance (8-12 steps)
```

**Implementation location:** `crates/weave-server/src/service/kanban_prompt.rs` (new module).

**Data sources:**
- `Task` struct (title, description, objective, labels, priority, column_id, board_id, session_id)
- `Column` struct (name, position, auto_trigger, specialist_id, runtime_kind, automation config)
- `Board` struct (columns list for ordering)
- `TaskStore` (lane sessions, artifacts)
- Specialist frontmatter (role, model_tier)

### 5.2 Kanban Tools for CLI Agents

Add kanban-interaction tools that the Claude Code agent can call through the existing `ToolRegistry` / `ToolExecutor` pipeline.

**Tools to implement:**

| Tool | Signature | Description |
|---|---|---|
| `move_card` | `{ cardId, targetColumnId }` | Move card to next column. Validates gates before executing. |
| `update_card` | `{ cardId, title?, description?, priority?, labels? }` | Update card metadata. |
| `update_task` | `{ taskId, scope?, acceptanceCriteria?, verificationCommands?, testCases? }` | Update structured task fields. |
| `list_artifacts` | `{ taskId }` | List existing artifacts for a task. |
| `provide_artifact` | `{ taskId, type, name, content, mimeType? }` | Attach an artifact (test results, code diff, logs). |
| `create_note` | `{ workspaceId, title, content, tags? }` | Create a note for planning/progress context. |

**Implementation location:** `crates/weave-server/src/tools/kanban/` (extend existing directory).

**Key design decisions:**

- `move_card` calls `check_transition_gates` before executing. If gates fail, the tool returns an error with the specific gate that failed — the agent can then address it.
- Tools are registered in `ToolRegistry` and subject to the session's `ToolProfile`. The `implementation` profile gets all kanban tools. The `planning` profile gets `update_task`, `update_card`, `create_note` but not `move_card`.
- Tools are available to all runtime kinds (Anthropic API native, Claude Code wrapped, Codex, OpenCode). The `ToolExecutor` dispatch is runtime-agnostic.

### 5.3 Column-Stage Awareness

Each column declares a `stage` enum that controls prompt variation and tool filtering.

**Stages:**

| Stage | Prompt emphasis | Tool restrictions |
|---|---|---|
| `backlog` | Planning, refinement, decomposition. No implementation. | No `move_card` to non-backlog columns. Read-only filesystem tools. |
| `todo` | Orchestration, dependency checking, execution planning. | `move_card` allowed to dev columns. |
| `dev` | Implementation, verification, commit. | All tools. Filesystem + kanban. |
| `review` | Verification, evidence collection, quality gate. | `move_card` to done only if gates pass. |
| `done` | Summary, cleanup. | No `move_card`. Read-only. |

**Implementation:** Add a `stage` field to the `Column` struct (new migration). The prompt builder reads `column.stage` and adjusts the instructions section and tool list accordingly.

**Backward compatibility:** Existing columns without a `stage` field default to `dev` (the most permissive stage). The migration backfills from column name heuristics (`Backlog` → `backlog`, `To Do` → `todo`, `In Progress` → `dev`, `Review` → `review`, `Done` → `done`).

### 5.4 Transition Gate Enforcement

Extend `check_transition_gates` to validate the full gate set before `move_card` executes.

**Gate types:**

| Gate | Source | Enforcement |
|---|---|---|
| Artifact gate | Column `automation.requiredArtifacts` | `list_artifacts` must show all required types present |
| Delivery gate | Column `automation.deliveryRules` | Task must have `committed_changes=true`, `clean_worktree=true` (checked via git status) |
| Contract gate | Column `automation.contractRules.requireCanonicalStory` | Task description must contain a parseable `yaml` code block |
| Checklist gate | Column `automation.requiredChecklist` | Task evidence must contain checked markdown items |
| Validator gate | Column `automation.validatorCommand` | Command output must be present in artifacts |

**Implementation:** `check_transition_gates` in `tools/kanban/move_card.rs` expands from its current minimal check to evaluate each gate type. The function returns `Ok(())` if all gates pass, or `Err(AppError::Validation { code, message })` with the specific failing gate.

**Gate mode:** Columns can set `gate_mode: "blocking"` (move rejected) or `gate_mode: "warning"` (move allowed but logged). Default is `blocking`.

### 5.5 Lifecycle Management

Add session supervision for kanban-triggered Claude Code sessions.

**Components:**

| Component | Behavior | Interval |
|---|---|---|
| **Watchdog timer** | Scans active kanban sessions for staleness (no SSE event in N seconds) | 30s |
| **Stall detection** | Session has no activity for > `stall_threshold` (default: 5 minutes) | on watchdog scan |
| **Recovery action** | Re-send the initial prompt with a "you appear stalled, continue" prefix | on stall detection |
| **Max retries** | Stop recovery after `max_recovery_retries` (default: 2) | per session |
| **Failure broadcast** | Emit `session_failed` SSE event on board channel after max retries | on max retries |

**Implementation location:** `crates/weave-server/src/service/kanban_lifecycle.rs` (new module).

**Key design decisions:**

- The watchdog runs as a tokio background task spawned at server startup (mirrors the orphan-reaping pattern in `service/startup.rs`).
- Recovery re-sends the prompt through `SessionService::send_prompt`, which persists the message and spawns a new streaming task. The CLI's resume mechanism handles continuity.
- The watchdog only supervises kanban-auto-spawned sessions (sessions with a linked `task_id`). Manually created sessions are not supervised.
- The stall threshold is configurable per column via `automation.stall_threshold_seconds`.

## 6. Implementation Sequence

### Phase 1: Rich Prompt (minimum viable coordination)

**Effort:** ~2 days
**Files:** `service/kanban_prompt.rs` (new), `service/kanban.rs` (modify `build_initial_prompt`)

1. Create `KanbanPromptContext` struct that gathers task, column, board, and session state.
2. Implement `build_kanban_prompt(task, column, board, session_state) -> String` with all sections from §5.1.
3. Replace `build_initial_prompt` call in `try_automate_lane` with `build_kanban_prompt`.
4. Add unit tests for each prompt section (gates, history, readiness).
5. Update specialist `.md` files to reference the new prompt context in their system prompts.

### Phase 2: Kanban Tools (bidirectional coordination)

**Effort:** ~3 days
**Files:** `tools/kanban/move_card.rs`, `tools/kanban/update_card.rs` (new), `tools/kanban/artifacts.rs` (new), `tools/mod.rs`

1. Implement `move_card` tool with gate validation.
2. Implement `update_card` tool.
3. Implement `update_task` tool (extend existing task-update logic).
4. Implement `list_artifacts` and `provide_artifact` tools.
5. Register all tools in `ToolRegistry` with appropriate `ToolProfile` bindings.
6. Add integration tests: agent calls `move_card` → gate check → card moves → SSE event broadcasts.

### Phase 3: Column-Stage Awareness

**Effort:** ~2 days
**Files:** `store/columns.rs`, `resources/migrations/` (new migration), `service/kanban_prompt.rs`

1. Add `stage TEXT NOT NULL DEFAULT 'dev'` column to `columns` table.
2. Migration backfills from column name heuristics.
3. Prompt builder reads `column.stage` and adjusts instructions + tool list.
4. `move_card` tool filters allowed target columns by stage.

### Phase 4: Gate Enforcement

**Effort:** ~2 days
**Files:** `tools/kanban/move_card.rs`, `store/columns.rs`

1. Extend `Column` struct with `automation` JSON field (artifact gates, delivery rules, contract rules, gate mode).
2. Implement gate evaluation in `check_transition_gates`.
3. `move_card` tool calls `check_transition_gates` before executing.
4. Add tests for each gate type (artifact present/missing, delivery satisfied/violated, contract valid/invalid).

### Phase 5: Lifecycle Management

**Effort:** ~2 days
**Files:** `service/kanban_lifecycle.rs` (new), `main.rs` / `lib.rs` (spawn background task)

1. Implement watchdog scanner (tokio interval task).
2. Implement stall detection (check last SSE event timestamp).
3. Implement recovery re-prompt.
4. Implement max-retries and failure broadcast.
5. Add integration test: create kanban session → simulate stall → verify recovery prompt → verify failure after max retries.

## 7. Schema Changes

### New migration: `012_column_stage_and_automation.sql`

```sql
-- Add stage column for column-stage awareness
ALTER TABLE columns ADD COLUMN stage TEXT NOT NULL DEFAULT 'dev';

-- Add automation config for gate enforcement
ALTER TABLE columns ADD COLUMN automation_json TEXT;

-- Backfill stage from column names (best-effort)
UPDATE columns SET stage = 'backlog' WHERE name = 'Backlog' AND stage = 'dev';
UPDATE columns SET stage = 'todo' WHERE name = 'To Do' AND stage = 'dev';
UPDATE columns SET stage = 'dev' WHERE name = 'In Progress' AND stage = 'dev';
UPDATE columns SET stage = 'review' WHERE name = 'Review' AND stage = 'dev';
UPDATE columns SET stage = 'done' WHERE name = 'Done' AND stage = 'dev';
```

### Column automation JSON shape

```json
{
  "requiredArtifacts": ["test_results", "code_diff"],
  "deliveryRules": {
    "requireCommittedChanges": true,
    "requireCleanWorktree": true
  },
  "contractRules": {
    "requireCanonicalStory": true
  },
  "gateMode": "blocking",
  "stallThresholdSeconds": 300
}
```

## 8. API Changes

### `POST /api/columns` and `PATCH /api/columns/:id`

Accept optional `stage` and `automation` fields. Existing callers are unaffected (defaults apply).

### `POST /api/workspaces/:wid/tasks/:tid/move` (the move_card tool endpoint)

Returns structured error when gates fail:

```json
{
  "error": {
    "code": "gate_failed",
    "message": "artifact gate: test_results required but not found",
    "gate_type": "artifact",
    "missing": ["test_results"]
  }
}
```

## 9. Testing Strategy

| Layer | What to test | How |
|---|---|---|
| Unit | Prompt section generation, gate evaluation logic, stage-to-tool filtering | Rust unit tests in `kanban_prompt.rs`, `move_card.rs` |
| Integration | `try_automate_lane` produces rich prompt, `move_card` tool validates gates, watchdog detects stall | Rust integration tests in `tests/` |
| Conformance | Claude Code agent receives kanban tools and can call `move_card` | Fake CLI harness emits `tool_use` for `move_card`; assert card moved in DB |
| E2E | Card moves to dev → agent receives prompt → agent calls `update_card` → agent calls `move_card` → card arrives in review | Manual with real Claude CLI + `./init.sh` verification |

## 10. Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Claude Code ignores kanban tools | Agent does filesystem work but never moves card | Rich prompt explicitly lists tools and instructions; watchdog detects stall |
| Gate enforcement blocks legitimate moves | Agent can't advance card | Gate mode `warning` option; clear error messages tell agent what's missing |
| Prompt too large for CLI context window | Agent truncates critical sections | Prioritize sections by stage; backlog gets readiness, dev gets gates |
| Watchdog false positives | Recovery prompt sent to active session | Stall threshold generous (5 min); check for any SSE event, not just tool calls |
| Schema migration breaks existing boards | Columns lose data | Additive migration only; defaults preserve existing behavior |

## 11. What This Enables

After this design lands:

- A Claude Code agent spawned by kanban **knows what column it's in** and **what it needs to do**.
- The agent can **update the card** with progress and **move it forward** when work is complete.
- Card moves are **validated against gates** — no more free-form moves without evidence.
- Stalled sessions are **detected and recovered** — no more permanently stuck cards.
- The same tool surface works for Codex (feat-058) and OpenCode (feat-059) when they land.

This turns the kanban from a trigger into a **coordination surface** — the board and the agent communicate through structured prompts and tools, not fire-and-forget messages.
