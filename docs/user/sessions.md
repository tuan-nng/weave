# Sessions

A session is one conversation thread with one provider. You type a prompt,
the agent streams a response and (usually) calls a few tools, and the whole
turn is recorded as messages you can scroll back through.

Sessions are the default working mode in Weave. If you are not sure which
feature to start with, start here.

## What sessions are for

- one main thread you can recover later (sessions are persisted, not
  ephemeral)
- streaming visibility into what the agent is doing while it is doing it
- tool-call observability — every `fs_read`, `shell_exec`, `git_log` etc. is
  shown as a block you can expand
- feeding work into the [Journey sidebar](./journey) and the
  [Kanban auto-trigger](./kanban)

## How to use it

### Open the Sessions list

Click `Sessions` in the left sidebar. The page lists every session in the
currently selected workspace, with status, specialist, model, and creation
time. The list updates live — new sessions appear without a refresh.

### Create a session

1. From `Home`, open a workspace. Or, from the `Sessions` list, click
   `+ New Session` next to the workspace name where you want to start a
   session.
2. In the modal:
   - **Provider** — required. Pick from the configured providers.
   - **Specialist** — optional. Pick one of the bundled specialists from
     the dropdown (e.g. `dev-crafter`, `review-guard`) to inject a system
     prompt. Leave on "No specialist" to skip. See
     [Specialists](./specialists) for the full list and what each one does.
   - **Model** — optional. Leave empty to use the provider's default.
3. Click `Create Session`. The session page opens automatically and is
   already ready to receive a prompt.

### Send a prompt

Type into the input at the bottom of the session page and press `Enter`
(Shift+Enter inserts a newline).

While the agent is streaming, you will see:

- the live text bubble growing token-by-token;
- tool-call blocks appearing as the agent invokes tools — each one shows the
  tool name, the input (collapsible), and the output once the tool returns;
- a "Stop" button in the page header. Click it to cancel mid-flight. The
  partial response is preserved.

When the agent finishes, the turn is persisted as a complete
`AssistantMessage` and the input clears.

### Read a tool call

Every tool call is a block in the conversation. Click the header to expand
or collapse it. While the tool is running, the block is blue with a spinner.
When it finishes, the block turns slate-grey and the output is shown.

Common tools you will see:

| Tool | What it does | When the agent uses it |
| --- | --- | --- |
| `fs_read` | reads a file from the workspace's codebase | "show me X", "what's in this file" |
| `shell_exec` | runs a shell command | "run the tests", "git log" |
| `git_*` | git status / diff / log / commit | "what changed", "commit this" |
| `task_*` | reads/writes the current kanban task | when the session is bound to a task |

### Open the Journey sidebar

The thin rail on the right edge of the session page is the toggle. Open it
to see decisions, errors, and a deduplicated list of files the agent
touched. See [Journey sidebar](./journey).

### Resume a session

Sessions never expire by themselves, but they auto-complete after 30
minutes of inactivity. To keep working in a session, just send another
prompt. To continue from a previous session's history, create a new session
and pass the old `parent_session_id` — the new session inherits the prior
turns as context.

### Cancel a session

Click `Stop` in the header. Useful when:

- the agent is going in a direction you did not want
- a tool call is taking too long
- you want to interrupt and steer

Cancelled sessions are marked `cancelled` in the list and can be reopened
to read what was produced so far.

### Delete a session

From the workspace overview or the `Sessions` list, click the row's delete
action. The session and all its messages are removed. The
[Journey sidebar](./journey) trace is also removed.

## Statuses you will see

| Status | What it means |
| --- | --- |
| `connecting` | the session row was created, the first turn has not started yet |
| `ready` | the agent is between turns and waiting for a prompt |
| `completed` | the last turn ended with `end_turn` (normal finish) |
| `cancelled` | you hit Stop, or the session timed out |
| `error` | the provider returned a non-retryable error |

A session that just finished a turn is still `ready` for the next prompt.
It does not flip to `completed` until you walk away.

## Common pitfalls

**"I hit `+ New Session` and the modal just sat there."** The provider
dropdown is empty because no provider is configured. Go to
[Providers](./providers) first.

**"The agent answered without reading any files."** Either your prompt did
not need files, or the working directory the session is using is empty.
Sessions default to the workspace root — if you have not registered a
[Codebase](./codebases), there is nothing for the agent to read.

**"My session timed out while I was reading."** 30-minute inactivity
timeout. Send a new prompt to revive it, or accept that the next session
will resume the history.

**"I want a fresh conversation, not a continuation."** Create a new
session. Do not pass `parent_session_id`. The list of sessions is your
manual archive.

## Read next

- [Journey sidebar](./journey) — what the agent actually did
- [Specialists](./specialists) — the role bindings you can pass at session
  creation
- [Kanban](./kanban) — sessions that get auto-created when a card moves
- [Common workflows](./common-workflows) — typical session patterns
