# Kanban UI Gaps — Findings from 2026-06-13 Walkthrough

**Status:** investigation report (not a design proposal)
**Reproducer:** the "start a kanban for my-vault, add a task, fire the orchestrator" flow, repeated end-to-end through the Weave UI with `agent-browser`. The flow completes but the resulting session is materially worse than the same flow run through the API.
**Audience:** the next session that picks up UI work — this doc is a shopping list of small fixes, sized for a single `feat-XXX` per item or grouped into one larger `feat-board-templates-and-stages` work item.

---

## TL;DR

The kanban board UI lets a user create a board, add columns, bind a specialist, add a card, drag the card, and watch the orchestrator session get spawned. But it silently degrades the orchestration outcome versus the same flow via the API in five distinct ways:

1. **No board template** — every column must be created by hand (5 modal opens minimum).
2. **Add Column modal is a strict subset of the server's column DTO** — `runtime_kind`, `stage`, and `automation` are server-receivable but not UI-exposable.
3. **No column reordering** — new columns always append to the right, and there is no drag handle, no edit modal, no up/down buttons.
4. **No codebase binding on columns or cards** — auto-spawned sessions get `cwd=None` and `codebase_id=None`.
5. **Provider selection is uncontrollable** — the column has `runtime_kind=null` (the UI default), so the server picks the first healthy provider for the kind, which may not be what the user wants.

Items 2–5 are downstream of the same root cause: **`AddColumnModal` (`web/src/app/pages/board/add-column-modal.tsx`) sends only `name`, `auto_trigger`, `specialist_id`** (lines 43–47), even though the server accepts more.

There are 11 specific findings below. Severity is rated for the orchestrator workflow specifically; some items (e.g. drag-only card move) are higher priority when considering the general UX.

---

## The two runs side by side

| | API run (earlier) | UI run (this session) |
|---|---|---|
| Board template | 5 columns, fully configured, one POST | 5 columns, one modal each (5×) |
| Column stages | `backlog` / `todo` / `dev` / `review` / `done` | All default to `dev` |
| `runtime_kind` on column | `claude-code` on every auto-trigger column | `null` on every column (UI doesn't expose) |
| `specialist_id` | `todo-orchestrator`, `dev-crafter`, etc. | Same — selectable in UI |
| Codebase binding | n/a (session manually set) | n/a — UI has no binding surface |
| Card → orchestrator session | `cwd=/mnt/data/works/my-vault`, `runtime=claude-code`, `provider=claude-code` | `cwd=None`, `runtime=anthropic-api`, `provider=anthropic` |
| Session prompt "move_card target" | "advance to Dev" (correct next stage) | "advance to Review" (wrong: the column-after-To-Do in UI order is "In Progress" but the prompt template uses `stage` not `position`) |

The orchestrator is "running" in both cases, but the UI-run session is blind to the filesystem and uses the wrong provider.

---

## Findings

### F-1. New Board modal has no template / no column field

**Severity:** Medium
**Files:** `web/src/app/pages/board.tsx:103` (comment confirms "Column creation is exposed via the AddColumnButton inside"), `web/src/app/pages/board/board-container.tsx:67` (`addColumnOpen` state)

**Symptom:** A freshly created board has zero columns. The user must `+ Add column` five times to set up the standard flow. A kanban board without columns is also visually broken — the page is just an empty `+ Add column` placeholder.

**Reproduction:**
1. `Kanban` → `+ New board in default` → enter name → `Create board`.
2. The new board page renders with no columns.

**Fix shape (not designed):**
- Add a template picker to the New Board modal: `Empty` / `Standard (Backlog/To Do/In Progress/Review/Done)` / `Custom`.
- For the Standard template, pre-populate the column list (5 `NewColumnSpec` entries with the bundled specialist bindings — see `docs/user/kanban.md:110-117` for the canonical mapping).
- The `CreateBoardRequest` already accepts a `columns: Option<Vec<CreateColumnInline>>` — `BoardStore::create` accepts the same shape, so the server side is ready.

---

### F-2. Add Column modal is a strict subset of the server's column DTO

**Severity:** **High** (root cause of F-4, F-5, and the "wrong next stage" prompt bug)
**File:** `web/src/app/pages/board/add-column-modal.tsx:20-149`

**Symptom:** The modal exposes only `name`, `auto_trigger`, and `specialist_id`. The server's `CreateColumnRequest` (`crates/weave-server/src/api/kanban.rs:77-94`) accepts:
- `name` ✅
- `position` ❌
- `specialist_id` ✅
- `auto_trigger` ✅
- `freeze_description` ❌
- `required_fields` ❌
- `required_artifact_types` ❌
- `runtime_kind` ❌
- `stage` ❌
- `automation` ❌ (the full `AutomationConfig` struct)

**Why it matters:** Of the missing fields, three are critical for the orchestrator flow:
- `runtime_kind` — without this, the server picks "any healthy provider" (see F-5).
- `stage` — without this, every column defaults to `dev` (see F-3), which breaks the "advance to next stage" copy in the orchestrator prompt.
- `automation` — the planned gate machinery (delivery/contract/checklist/validator gates) is unreachable from the UI.

**Fix shape:**
- Add `Stage` picker (radio: `Backlog` / `To Do` / `In Progress` / `Review` / `Done`) with `To Do` as the default for new auto-trigger columns.
- Add `Runtime` picker (dropdown of registered `RuntimeKind`s: `claude-code`, `codex`, `opencode`, `anthropic-api`).
- The `automation` config is a separate feature; F-2 only requires the simple fields. (Track `automation` UI as a separate ticket.)
- `freeze_description`, `required_fields`, `required_artifact_types` are advanced knobs — gate behind an "Advanced" disclosure or defer to a follow-up.

---

### F-3. All columns default to `stage: dev`; no stage picker in the UI

**Severity:** **High**
**Files:** `crates/weave-server/src/store/columns.rs:530` (`ColumnStage::from_str(&stage_str).unwrap_or_default()`), `crates/weave-server/src/api/kanban.rs:367-375` (request handler — also `unwrap_or_default()`), `web/src/app/pages/board/add-column-modal.tsx:43-47` (modal never sends `stage`)

**Symptom:** Every column created through the UI has `stage: dev`. The orchestrator prompt template (`service/kanban_prompt_ctx`) uses `stage` to compute the "next stage" line ("When planning is complete, call move_card to advance to Dev."), so with everything at `dev` the prompt always says "advance to Review" — regardless of the column's actual position in the UI.

**Reproduction:**
```sh
curl -s "http://localhost:3000/api/workspaces/.../boards/..." | jq '.data.columns[] | {name, stage}'
# Every column has stage: "dev"
```

**Why it matters:** The user reading the orchestrator's initial prompt sees "advance to Review" after a To Do card with `todo-orchestrator`, but the next column in UI position is `In Progress` (or, in my run, `Done` because I created them out of order). The agent will dutifully call `move_card` and the system will move it to whatever `dev` maps to — which after a move to `In Progress` is `review`, not `done`. The "next stage" prompt line is meaningless until `stage` is settable from the UI.

**Fix:** F-2 (stage picker) closes this. Verify by adding a regression test that creates a column with `stage=todo` and asserts the assembled prompt says "advance to Dev" (not "Review").

---

### F-4. No codebase binding on columns or cards

**Severity:** **High**
**Files:** No binding surface exists. `kanban-card.tsx` shows no codebase selector. `add-column-modal.tsx` has no codebase selector. `crates/weave-server/src/api/kanban.rs:496-541` (the move handler) reads no `codebase_id` from the request.

**Symptom:** When the orchestrator fires, the spawned session gets `cwd: None` and `codebase_id: None`. The session can theoretically run, but the agent has no filesystem context to work in. The my-vault codebase is registered in the workspace but the session never sees it.

**Reproduction:**
```sh
# After a UI drag from Backlog → To Do
curl -s "http://localhost:3000/api/sessions/<sid>" | jq '.data | {cwd, codebase_id, status}'
# cwd: null, codebase_id: null
```

**Why it matters:** Without `cwd`, the agent can't read the codebase, can't run `ls`, can't operate on files. The session will fail at the first tool call that needs a filesystem path. The previous `todo-orchestrator` session on the same task (run via API) ended in `status: error` (per `PROGRESS-archive.md`) — likely related.

**Fix shape (requires a design choice):**
- **Option A (column-level binding):** each column has an optional `codebase_id`. All sessions spawned in that column inherit the codebase. Simple but coarse — a "Vault" board with two codebases (e.g. backend + frontend) can't model both.
- **Option B (card-level binding):** the card detail panel (`task-detail-panel.tsx`) gets a "Codebase" dropdown. The card carries the binding, sessions inherit. Matches how the card already carries its `description` and `acceptance_criteria`. **Recommended** — matches user mental model and existing card-as-payload architecture.
- **Option C (workspace default):** workspace gets a `default_codebase_id`. Used as fallback when the card doesn't specify. Easy, but doesn't model multi-codebase boards.

Either way, the session spawn in `try_automate_lane` (`crates/weave-server/src/service/kanban.rs:82-...`) needs to read the binding and pass it to the new session's `create_session` call. The session already supports `codebase_id` on insert (the existing `c3c7eacb-...` session in the DB has it).

---

### F-5. Provider selection is uncontrollable from the UI

**Severity:** Medium (same effect as F-2's missing `runtime_kind` field — listed separately because the symptom is observable in the spawned session and the user will be confused)
**Files:** `crates/weave-server/src/service/kanban.rs:try_automate_lane` (provider pick logic), `web/src/app/pages/board/add-column-modal.tsx` (no `runtime_kind` exposed — same root cause as F-2)

**Symptom:** Column has `runtime_kind=null` because the UI doesn't expose the field. The server picks "any healthy provider for the requested kind." In this run, the anthropic HTTP provider was selected over the claude-code CLI provider. Both were healthy. The user has no way to express "I want claude-code, not anthropic-api."

**Reproduction:** See the earlier diff — same `task_id`, same column, two different providers based purely on which field the API call included.

**Fix:** Same as F-2 — expose `runtime_kind` as a picker. Possibly **defer** if F-2 is the only one that ships; F-5 will follow.

---

### F-6. No column reordering

**Severity:** **High**
**Files:** `web/src/app/pages/board/board-column.tsx` (column is `useDroppable` for cards but not `useSortable` for itself), `web/src/app/pages/board/board-container.tsx` (no column-DnD context), `crates/weave-server/src/api/kanban.rs:109` (`PATCH /api/columns/{id}` accepts a `position` field but the UI never sends it)

**Symptom:** Adding columns always appends to the rightmost position. In my run I created columns in this order: Backlog, To Do, In Progress, Done, Review — the board ended up as `Backlog → To Do → In Progress → Done → Review`, which is semantically wrong (Done before Review). The only way to fix it is to delete and recreate.

**Why it matters:** `position` exists in the schema and the server's `PATCH /api/columns/{id}` accepts it (`crates/weave-server/src/api/kanban.rs:96-114`). The frontend just doesn't call it.

**Fix shape:**
- Add a column-header drag handle (the same `@dnd-kit` `useSortable` already used for cards).
- On drag-end, `PATCH /api/columns/:id` with `{ "position": <midpoint> }`. (Server rebalances if positions get too close — the card-level equivalent is in `board-container.tsx:112-122`.)
- Add a "Move left" / "Move right" button pair in the column header as a keyboard-accessible fallback. The disabled `+ Card` button in the header (F-11) could even be replaced with a `⋯` menu that includes "Move left / Move right / Edit / Delete."

**Reproduction:**
1. Create a board, add 5 columns in the order `Backlog, To Do, In Progress, Done, Review`.
2. Observe the rendered order: `Backlog, To Do, In Progress, Done, Review` (i.e. Done before Review).
3. Try to fix: no drag handle, no edit modal, no context menu. The only recourse is to delete the wrong column and re-add.

---

### F-7. Add column modal: specialist dropdown is alphabetically sorted with no hints

**Severity:** Low
**File:** `web/src/app/pages/board/add-column-modal.tsx:115-120`

**Symptom:** The dropdown shows `backlog-refiner`, `dev-crafter`, `done-reporter`, `review-guard`, `todo-orchestrator` alphabetically. The user has to know which specialist fits which column. The Specialist API (`/api/specialists`) returns `description`, `tool_profile`, and `tags` for each — none of which is shown in the picker.

**Fix:** Show the description as a secondary line under each option. The data is already on hand (`useSpecialists()` returns the full record).

**Low priority** — the docs (`docs/user/specialists.md`) explain the mapping, and the bundled specialists are well-known. F-7 is a polish item.

---

### F-8. Drag-and-drop is the only way to move a card

**Severity:** Medium (accessibility)
**File:** `web/src/app/pages/board/kanban-card.tsx:30-43` (entire card is `useSortable` listener)

**Symptom:** A user who cannot use a pointing device has no way to move a card. The card detail panel (`task-detail-panel.tsx`) has no "Move to..." action. There is no keyboard shortcut, no `⋯` menu on the card.

**Fix:** Add a "Move to..." button in the card detail panel. Open a small popover with the list of columns (the data is already in `useBoard()`).

---

### F-9. Horizontal scroll on the board is invisible at default zoom

**Severity:** Low
**File:** layout under `web/src/app/pages/board/board-container.tsx` (the `main` element is the scroll container)

**Symptom:** On a 1280×720 viewport, the 5-column board overflows horizontally. There is no visible scrollbar, no scroll affordance, no gradient mask. In my run, the `Review` column was rendered off-screen to the right and I had to call `eval` to scroll to it.

**Fix:** Add a subtle right-edge gradient mask when there is more content off-screen. `tailwindcss` doesn't have a built-in; either use `mask-image: linear-gradient(to right, black 95%, transparent 100%)` or render a "scroll for more" hint button.

---

### F-10. Card detail panel does not surface the specialist / runtime / stage

**Severity:** Medium
**File:** `web/src/app/pages/board/task-detail-panel.tsx` (the panel renders Title / Description / Status / Acceptance criteria / Completion summary / Verification report — see `docs/user/kanban.md:63-76`)

**Symptom:** The card panel has no read-only display of the binding state (which specialist / runtime / stage the column has). A user who lands on a card mid-flow cannot tell which specialist will pick it up when they drag it. The server has all the data; the panel just doesn't render it.

**Fix:** Add a small "Lane" footer to the panel: `Lane: To Do (todo-orchestrator, claude-code, stage: todo)`. Pull from the `useBoard()` query (column record is already loaded).

---

### F-11. The page-header `+ Card` button is permanently disabled

**Severity:** Low (dead UI)
**File:** `web/src/app/pages/board/board-container.tsx:161` area (the `<button disabled>+ Card</button>` in the snapshot)

**Symptom:** The `+ Card` button in the top right of the board page is always disabled. The user must add cards from a column's `+ Add card` placeholder. The docs (`docs/user/kanban.md:55-59`) say `+ Card` "drops a card into the first column with a default title" — that behavior isn't wired up.

**Fix:** Either wire it up (`+ Card` picks the leftmost column, opens the Add card modal with that column pre-selected) or remove the button. The current state is misleading.

---

## Suggested sequencing

The 11 findings cluster into roughly 4 work items:

| Work item | Findings | Why group |
|---|---|---|
| **Board templates** | F-1 | Single feature, low coupling. |
| **Full column DTO in the UI** | F-2, F-3, F-5, F-7, F-10 | One change to `add-column-modal.tsx` plus the card detail panel. Closes the most-degrading UX gap (wrong provider, wrong stage). |
| **Column reordering** | F-6, F-9 | Drag handle + scroll affordance. Independent of column-DTO work. |
| **Codebase binding** | F-4, F-8, F-11 | Card-level codebase selector (F-4) is the most useful; the others are smaller cleanups that pair with it. |

A single `feat-board-templates-and-stages` work item covering all of the "full column DTO" cluster would be a reasonable single-PR scope, and would let the next session's orchestrator walkthrough succeed end-to-end.

---

## What was tested

- The "create codebase + create board + add 5 columns + add card + drag to To Do" path was completed via UI only (no API calls during the create flow). The card moved, the orchestrator session spawned.
- The session is `status: ready` and the user can type follow-up prompts. The session did **not** run end-to-end (the agent was not sent a follow-up prompt); the bug surface in this report is the **initial state** of the spawned session, which is fully determined by the move-time context.

## What was not tested

- The follow-up prompts and tool calls. The agent's behaviour on the wrong `cwd` / wrong provider is unverified here.
- Multi-codebase boards (Option A/B/C in F-4 is a design question — no implementation has been chosen).
- The `Done` column's `done-reporter` (I did not create a card and move it through to Done).
- Other specialists (`backlog-refiner`, `review-guard`) — only `todo-orchestrator` was exercised.

---

## Out of scope (noted, not fixed)

- The orchestrator specialist itself — its `system_prompt` and tool surface look correct (the prompt arrived intact, the agent got the right instructions). The bugs are all upstream (column binding, codebase binding, provider selection).
- The card-side session binding (the `Agent` link on the card) works correctly.
- The board SSE channel works (the new card appeared on the page without a refresh, and the move broadcast was picked up by other tabs in the screenshot).

---

## Reference

- Walkthrough session: 2026-06-13
- Run identifiers:
  - Workspace: `default` (id `5a7675ff-...`)
  - Codebase: `my-vault` (id `2a1419dc-c732-4b32-a8d2-2c935c2f9816`)
  - Board: `Vault` (id `8842e716-d10a-4534-aaa9-5712050c6fbb`)
  - Card: `check what to do today` (id `d96e6e04-...`)
  - Orchestrator session (UI run): `e5d7de56-f8fa-46c4-9126-07406a69d8bf` (`cwd=None`, `runtime=anthropic-api`)
- Pre-existing context: see `PROGRESS-archive.md` § feat-051/055/063/065 for the lane-automation and stage design that these findings build on. The "previous `todo-orchestrator` session ended in `error`" note there (re: the API run) is consistent with the `cwd=None` pattern this UI run produced.

---

## Appendix B — Second walkthrough (2026-06-13, post-feat-068)

After feat-068 landed, the same walkthrough was repeated. The 11 findings above are all closed. **Five new small findings** surfaced, addressed in feat-069:

- **F-12 — Title flash on TaskDetailPanel open.** The previous `useEffect(() => setDraft(...))` design produced a one-frame empty title on first paint (the user sees "Untitled" or a blank box flicker before the effect runs). Replaced with a `useState` lazy initializer + a `lastSyncedTaskIdRef` guard so re-opens and SSE `task_updated` patches don't re-overwrite in-flight edits.
- **F-13 — Save button affordance.** Disabled Save / Delete buttons had no visible reason. Added `title` tooltips explaining *why* the button is disabled ("No task selected" / "No changes to save" / "Title is required" / "Saving…") plus `disabled:cursor-not-allowed` for the cursor story.
- **F-14 — Agent-needs-input notification (headline fix).** The Agent pill on a card was a static "Agent" badge. The user had no way to tell whether the agent was actively working or waiting on their next prompt. Backend: `Session` struct now carries derived `last_message_role` (Option<String>) + `awaiting_user_input` (bool) computed via SQL scalar subqueries — no migration needed, derived not stored. New endpoint `GET /api/workspaces/{wid}/sessions/awaiting-input` returns the workspace's paused-agent list. Frontend: `useAgentStatus` hook + `describeSession()` state machine. KanbanCard pill now switches tone + label: "Running" (blue) / "Needs input" (rose, with dot + ring + rose border) / "Error" (rose) / "Cancelled" (slate) / "Completed" (emerald).
- **F-15 — Codebase dropdown on Add Card modal.** The codebase binding existed on the column (feat-068) and on the task detail panel (F-4) but not on the card-creation flow. New card couldn't be pinned to a specific codebase at creation time. Added a Codebase dropdown to `AddCardModal`, plumbed through `BoardContainer → onCreateCard → createCard`. Backend `CreateCardRequest.codebase_id` widens + `TaskStore::create` validates workspace scope (cross-workspace ids return 404 — `CodebaseStore::get_in_workspace` is the workspace-scope guard).
- **F-16 — Sessions nav badge.** No way to see at-a-glance which workspaces have a paused agent waiting. Added a `usePendingInputCount()` hook in the layout that sums per-workspace counts, renders a small rose pill next to the "Sessions" label. 10-second refetch keeps it fresh without spamming the API.

**Verification:** 896 Rust + 153 frontend tests pass (was 894 + 147; +2 Rust for `list_awaiting_input_filters_correctly` + `test_create_card_with_codebase_id_round_trips`; +6 frontend for the `describeSession` state machine in `use-agent-status.test.ts`). `./init.sh` 3-layer gate green.
