# Specialists

A specialist is a named role: a system prompt, a model preference, and a
description of what the role is for. Specialists are how Weave turns
"the agent" from one monolithic thing into a small team of differently
prompted workers.

You will see specialists in two places: the `Specialist` field in the
`+ New Session` modal, and the `Specialist` dropdown when binding a
Kanban column.

## What they are for

- tailoring the agent's system prompt to the kind of work it is doing
- giving different stages of a flow (refine → plan → build → review →
  report) different personas
- letting a single provider serve many roles without rewriting prompts
  per session

Specialists are not separate model servers. A specialist is just a
prompt injection on top of the model your session is already using.

## The bundled specialists

Weave ships with five specialists, defined as Markdown files with YAML
frontmatter under `resources/specialists/`:

| ID | Role | When to use |
| --- | --- | --- |
| `backlog-refiner` | turns a rough card into a clear story with acceptance criteria | Backlog column, or the first prompt of a session that is breaking down a vague task |
| `todo-orchestrator` | validates a refined story and adds an execution plan | To Do column, or right after the refiner finishes |
| `dev-crafter` | implements changes, runs verification, commits | In Progress column, or any session that should write code |
| `review-guard` | verifies acceptance criteria, rejects or approves | Review column, or a session that should check the work of another |
| `done-reporter` | writes the completion summary | Done column (not auto-triggered; the review guard or the operator decides when to run it) |

These are *defaults*. They are intended to be replaced or extended. The
files on disk are the source of truth — open one to read the exact
prompt.

## How to use a specialist

### In a session

1. Open `+ New Session`.
2. In the `Specialist` field, type one of the IDs above (e.g.
   `dev-crafter`). The field is a free-text input, not a dropdown — the
   value is sent verbatim to the server.
3. Create the session. The system prompt is injected before the first
   turn.

Leave the field empty for an unprompted session. That is what you want
for ad-hoc Q&A.

### On a Kanban column

1. Add or edit a column.
2. Toggle `Auto-trigger` on. The `Specialist` dropdown becomes
   enabled.
3. Pick a specialist. Cards dragged into this column will create a
   session with that specialist as the system prompt.

See [Kanban](./kanban) for the full flow.

## Choosing the right specialist

| You want to... | Pick |
| --- | --- |
| take a vague idea and turn it into a buildable card | `backlog-refiner` |
| decide what files to change and in what order | `todo-orchestrator` |
| actually edit files, run the test suite, and commit | `dev-crafter` |
| check that someone else's work meets the acceptance criteria | `review-guard` |
| produce a summary suitable for sharing with a stakeholder | `done-reporter` |

Most users pick a specialist per *column*, not per *session*. That way
the role stays consistent across many cards.

## What the system prompt actually does

A specialist's Markdown file looks like this:

```markdown
---
name: Dev Crafter
model: sonnet
description: Implements changes within task scope
---

You are a dev crafter. Stay within task scope...
```

When a session is created with `specialist_id: "dev-crafter"`, the
frontmatter is parsed and the body is prepended to the agent's system
prompt for every turn. The `model` field is informational in v1; the
session's actual model is still the one picked at session creation.

## Common pitfalls

**"I typed a specialist name and the session feels no different."**
Either the name is misspelled (the field is free text and the server
does not auto-suggest) or the file is missing from
`resources/specialists/`. Check the binary's logs for a "specialist
not found" warning.

**"The agent is following the wrong role."** A session keeps its
specialist for its whole life. If you want a different role, start a
new session — do not try to "switch" mid-thread.

**"I want my own specialist."** Drop a Markdown file in
`resources/specialists/` with the right frontmatter, restart the
binary, and the new ID becomes available in the dropdown. The file is
hot-loaded on startup, not at request time.

## Read next

- [Sessions](./sessions) — the surface that consumes a specialist
- [Kanban](./kanban) — the surface that binds a specialist to a column
- [Common workflows](./common-workflows) — typical specialist
  pairings
