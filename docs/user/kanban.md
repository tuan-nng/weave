# Kanban

A Kanban board is a delivery flow. Columns are stages, cards are tasks,
and moving a card into a column with a bound specialist **auto-triggers**
an agent session that works that card.

This is the feature that turns Weave from "an agent chat" into "a delivery
platform" — the same card moves through refinement, planning, building,
and review without you writing a single orchestration script.

## What it is for

- breaking a backlog into well-formed cards with acceptance criteria
- moving work through explicit stages (Backlog → To Do → In Progress →
  Review → Done) with visible state
- letting column bindings decide which specialist picks up the card at
  each stage
- reviewing the result before declaring the work done

## How to use it

### Open the Boards list

Click `Kanban` in the left sidebar. The page lists every board in the
current workspace. Each row shows the board name, the first 8 characters
of its ID, and the creation date.

### Create a board

1. From the `Kanban` list, click `+ New board in <workspace>`.
2. Type a name (e.g. `Product Sprint Q3`). Click `Create board`.
3. The new board opens at `/workspaces/:wid/boards/:bid`. The board is
   empty by default — the first column is created on the fly when you
   click `+ Card`, and you can add more columns with the `+` button at
   the end of the row.

### Add a column

1. On the board page, click the `+` button at the right end of the column
   row.
2. In the modal:
   - **Name** — the column's display name (e.g. `In Progress`).
   - **Auto-trigger** — toggle on if you want Weave to create a session
     when a card lands in this column.
   - **Specialist** — required when Auto-trigger is on. Pick one of the
     bundled specialists (`backlog-refiner`, `todo-orchestrator`,
     `dev-crafter`, `review-guard`, `done-reporter`). The dropdown is
     disabled until you toggle Auto-trigger on.
3. Click `Create column`. The column appears at the right end of the row.

### Add a card

Two ways:

- **From the column header**, click the `+` button on a specific column.
  A modal asks for a title and an optional description.
- **From the page header**, click `+ Card` to drop a card into the first
  column with a default title (`New card`). Edit the title in the
  detail panel.

### Open a card's detail panel

Click any card. A slide-over opens from the right with five editable
fields:

| Field | Used for |
| --- | --- |
| **Title** | the card's display name (required) |
| **Description** | free-form context for the agent that picks up the card |
| **Status** | the lifecycle state (`pending`, `in_progress`, `blocked`, `completed`, `cancelled`) — usually set by the agent |
| **Acceptance criteria** | what "done" means for this card; the agent reads this when starting work |
| **Completion summary** | written by the agent when it finishes, summarising what it did |
| **Verification report** | written by the review specialist when it inspects the result |

Edits are local until you click `Save`. The button is disabled when
nothing changed. To delete the card, click `Delete` in the panel header.

### Move a card

Drag a card horizontally between columns. Drop targets are highlighted
as you drag:

- Drop on a column header to append the card at the end of that column.
- Drop on another card to insert just above it.

The card's position is preserved. The change is broadcast over SSE, so
other browser tabs watching the same board update in real time without a
refresh.

### Watch an auto-triggered session run

When a card lands in a column with `auto_trigger=true` and a bound
specialist:

1. The server creates a new session.
2. The session's initial prompt is auto-generated from the card's title,
   description, and acceptance criteria.
3. The card's `session_id` is set, so you can open the session directly.
4. A `session_started` event is broadcast on the board's SSE channel.

To open the running session, click the card's detail panel and look for
the `Session` link. The card's `Status` chip updates to `in_progress`
while the agent works and `completed` when it ends.

## The default column shape

If you create a board and use only the auto-bindings, the natural
five-stage shape is:

| Position | Name | Bound specialist | Auto-trigger |
| --- | --- | --- | --- |
| 0 | Backlog | `backlog-refiner` | on |
| 1 | To Do | `todo-orchestrator` | on |
| 2 | In Progress | `dev-crafter` | on |
| 3 | Review | `review-guard` | on |
| 4 | Done | `done-reporter` | off |

The `done-reporter` column is intentionally **not** auto-triggered. The
review specialist that lands a card there is the one that decides
whether the work is actually done, and a second agent rewriting a
completion summary after the fact would be noise.

You are not required to use this shape. The bindings on each column are
independent. Pick what fits the work.

## When to use Kanban (vs. just sessions)

Use Kanban when:

- the work is repeatable — many cards with the same shape
- you want explicit gates between stages (especially `Review` and `Done`)
- you want the agent's output to land in a structured place, not float
  in chat history
- you want to see the whole flow at a glance

Use plain [Sessions](./sessions) when:

- the task is exploratory and you do not know its shape yet
- you are debugging or recovering
- the work is one-off

A common pattern is: use Sessions to scope and understand a project, then
move to Kanban once the recurring work shows up.

## Common pitfalls

**"I moved a card to In Progress but no session was created."** Either
the column does not have `auto_trigger` on, or it has `auto_trigger` on
but no `specialist_id` set. Edit the column and bind one.

**"Two sessions were created for the same card."** The most common
cause is dragging a card through an auto-triggered column twice in a row
(by accident). Each crossing creates a new session. If you do not want
that, untick Auto-trigger on the source column or move the card
deliberately once.

**"The agent finished but the card is still `in_progress`."** The
specialist that ran did not write `completion_summary` or did not flip
the status. Open the session, read the journey, and either write the
summary manually or move the card back and let another specialist
finish it.

**"I cannot drag a card."** The card detail panel might be open. Click
outside the panel first, then drag.

## Read next

- [Specialists](./specialists) — the role each column binding
  actually invokes
- [Sessions](./sessions) — what an auto-triggered session looks like
  end-to-end
- [Common workflows](./common-workflows) — typical Kanban patterns
