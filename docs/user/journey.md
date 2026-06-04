# Journey Sidebar

The Journey sidebar is the right-hand panel inside a session page. It
turns the raw tool-call stream into a human-readable timeline: what the
agent decided, where it stumbled, and which files it touched.

If the chat is what the agent said, the journey is what the agent did and
why.

## What it is for

- auditing a session after the fact — "did it actually read the file I
  asked about?"
- seeing the agent's reasoning path, not just the final answer
- copying a file path the agent worked on, without scrolling the chat

The journey is built from the same trace events that drive the API
`/sessions/:sid/trace` endpoints. The sidebar is the human surface for
that data.

## How to use it

### Open the sidebar

The session page has a thin rail on the right edge with a chart icon.
Click it. The aside animates from 40 px wide to 360 px wide, and the chat
column resizes to fill the freed space.

### Close the sidebar

Either click the `×` in the sidebar header, or click the chart icon in the
rail again. The rail stays visible so you can re-open it.

### Read the Decisions & Errors section

The top half of the sidebar, in chronological order:

- **Decision** rows are orchid. Each one is the agent's reasoning at a
  point in the run — usually a short summary extracted from the model's
  thinking. Click a row to expand the full text.
- **Error** rows are red. They are not expandable. Each one is a real
  failure the agent hit (bad path, missing tool, rejected call) and either
  recovered from or did not.

A typical session shows 1–5 decision rows, not one per token. The system
already coalesces streaming thinking deltas into a single decision per
reasoning pass, so a long agentic loop with 20 tool calls still reads as a
handful of "I looked at X, then I tried Y, then I wrote Z" beats.

### Read the Files section

The bottom half deduplicates every file the agent touched, regardless of
how many times:

- **read** chips are slate
- **write** chips are blue
- **create** chips are emerald
- **delete** chips are red
- a count of touches sits next to each file

Click a file path to copy it to the clipboard. A small "Copied!" tooltip
flashes on confirmation.

### Reload the trace

The sidebar data is fetched once on open and is not auto-refreshed
mid-session. If the agent is still running, close and reopen the sidebar
to pick up the latest events.

## When the sidebar is empty

An empty sidebar is meaningful. It usually means:

- the agent answered from its own knowledge without calling any tool;
- the session was just created and the first prompt has not been sent
  yet;
- the trace flush has not happened yet (every session batches trace
  events and flushes in the background; you can wait a few seconds and
  reopen the sidebar).

If the sidebar is empty after a long session that clearly did work, the
most likely cause is that the session is bound to a different workspace
than the one you are viewing from. The journey is workspace-scoped.

## What it is *not*

- The sidebar is **not** a debug log. The full trace (raw tool inputs,
  raw outputs, exact timing) is on `GET /api/sessions/:sid/trace`.
- The sidebar is **not** a chat history. Read the chat column for that.
- The sidebar is **not** searchable in v1. Use the trace endpoint if you
  need to filter events.

## Common pitfalls

**"I see a tool call in the chat but it is not in the Files section."**
The Files section is a deduplicated list of paths the agent *touched* —
read, write, create, or delete. A tool call that did not touch a file
(e.g. `shell_exec` running `ls`, or a kanban tool that moved a card) is
correctly absent.

**"Decisions say 'User asked the agent to...' even though I never saw
that text."** Decisions are written from the agent's perspective at the
time the reasoning was emitted. The first decision of a session is often
a re-statement of the prompt, because the agent is parsing the task
before acting.

**"The sidebar is open but it is showing 'Loading' forever."** The trace
endpoint is reachable but the session has no trace rows yet — usually
because the session was just cancelled. Close and reopen the sidebar
after a few seconds.

## Read next

- [Sessions](./sessions) — the chat surface the sidebar lives next to
- [Specialists](./specialists) — the role binding that affects what
  decisions and tool calls you see
- [Common workflows](./common-workflows) — when to read the journey vs.
  read the chat
