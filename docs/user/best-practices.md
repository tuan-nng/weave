# Best Practices

A short set of rules that keep Weave useful. None of them are
enforced by the UI; they come from the patterns that survive in real
projects.

## Start with the smallest useful mode

Use [Sessions](./sessions) for the first thing in a new workspace. Do
not open a Kanban board with five specialist-bound columns until you
have one or two successful session runs to anchor the choices.

## Configure one working provider first

Do not front-load every provider, model, and default. One provider
path that can execute real work is enough for a first successful run.
Add a second provider only when you have a concrete reason — cost,
latency, A/B testing, air-gapped fallback.

## Keep work workspace-scoped

Treat the workspace as the main operating boundary. Repositories,
sessions, boards, and codebases should all live inside the workspace
that owns the project, not in a "scratch" workspace you use for
everything.

## Use Kanban for repeatable delivery

If you want clear review boundaries, acceptance criteria, and handoff
visibility, move the work to [Kanban](./kanban) instead of trying to
simulate that process in one long session. The column binding to a
[Specialist](./specialists) is what makes the difference — without it,
Kanban is just a colourful to-do list.

## Bind specialists to columns, not to ad-hoc sessions

A specialist is most useful when it is bound to a Kanban column and
fires automatically. Ad-hoc specialist selection per session works,
but it is harder to keep consistent across many runs.

## Read the Journey sidebar, not just the chat

The chat shows what the agent *said*. The [Journey](./journey) shows
what the agent *did*. For any non-trivial task, open the sidebar
after the run. If `Files` is empty, the agent answered from its own
knowledge and you should not trust the result for codebase-specific
work.

## Cancel early, do not let bad runs ride

If a session is going the wrong way, hit `Stop`. The partial response
is preserved and you can re-prompt with a clearer instruction. Letting
a bad run continue just because you feel bad about cancelling wastes
tokens and produces more text to read.

## One card, one specialist, one outcome

A [Kanban](./kanban) card should pass through each column once. If a
card bounces back to `In Progress` from `Review`, the reviewer's
`verification_report` should say *what* needs to change, not just
"looks wrong". Re-dragging the same card with the same description
guarantees the same outcome.

## Register codebases once, refer to them by path in prompts

Sessions do not auto-bind to a [Codebase](./codebases). They just have
a working directory. When you prompt, name the absolute path:
*"Read `/path/to/repo/Cargo.toml`"*, not *"Read the project config"*.
The latter forces the agent to guess, and the guess is wrong whenever
there is more than one codebase registered.

## Do not delete providers while sessions are running

The provider store does not cascade. If you delete a provider that
has live sessions, those sessions keep working against the
configuration they were created with until they end, but new sessions
cannot pick that provider anymore. Delete providers only between
runs.

## Keep reference material out of the critical path

The internal docs in `docs/` are for engineers extending Weave, not
for first-time users. If a teammate is trying Weave for the first
time, point them at this section (`docs/user/`) — it covers what
each feature is for and how to use it, in the order a new user
needs it.

## When in doubt, restart the session

If a session is in a weird state — the SSE stream dropped, the
"Stop" button is unresponsive, a tool call has hung — do not try to
debug it. Open a new session with the old `session_id` as
`parent_session_id` and continue. The history is preserved and the
new session will pick up where the old one left off.

## Read next

- [Common workflows](./common-workflows) — concrete patterns that
  apply these rules
- [Sessions](./sessions) — the most-used surface
- [Kanban](./kanban) — the highest-leverage surface once a flow is
  well-defined
