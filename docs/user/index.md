# Use Weave

This section explains what each part of Weave does, and how to get useful work
out of it once the binary is running and one provider is configured.

Weave is a **web-based multi-agent coordination platform**. It is not a chatbot
shell. Sessions, boards, codebases, and specialists are first-class objects so
that work stays attached to explicit product structure instead of getting lost
inside one long-running chat.

## The five things you can do

| Feature | Best for | Where in the UI |
| --- | --- | --- |
| [Workspaces](./workspaces) | keeping unrelated projects isolated; switching context | left sidebar → Home |
| [Providers](./providers) | connecting an Anthropic-compatible model backend | left sidebar → Settings |
| [Sessions](./sessions) | the default "ask the agent to do a thing" path | left sidebar → Sessions |
| [Kanban](./kanban) | repeatable delivery flow with lane automation | left sidebar → Kanban |
| [Codebases](./codebases) | pinning git repos so the agent can see real source | left sidebar → Codebases |

Two more features live *next to* the things you do, not as separate top-level
pages:

- [Journey sidebar](./journey) — appears inside a Session, shows the agent's
  decisions and the files it touched.
- [Specialists](./specialists) — appear as named roles (e.g. `dev-crafter`,
  `review-guard`) when you create a session or bind a Kanban column.

## Recommended first 5 minutes

If you only have five minutes, do this and stop:

1. Open `Settings` and add one provider (Anthropic, an OpenAI-compatible proxy,
   or a local model endpoint). The form wants a name, a base URL, an API key,
   and a default model.
2. Open the default workspace. The UI lands you on the workspace overview
   when you click the workspace row from `Home`.
3. Click `+ New Session`, pick your provider, leave `Specialist` empty, and
   send a real prompt: *"Summarize the structure of the repository at
   `/path/to/your/code`."* Use that as your smoke test.
4. Open the **Journey sidebar** on the right of the session page. It shows
   what the agent did and which files it looked at while answering.

If that round trip works, the rest of Weave is just variations on that loop.

## Recommended order after that

1. [Workspaces](./workspaces) — set up the workspace boundary for the project
   you'll be working in.
2. [Sessions](./sessions) — learn the chat surface, the cancel button, the
   journey sidebar, and how to resume a session.
3. [Kanban](./kanban) — move to this when you have repeated work (a backlog of
   similar tasks) and want lane automation.
4. [Codebases](./codebases) — register a git repo so sessions and specialists
   have a real working directory to operate on.
5. [Providers](./providers) — read this in detail only when you need a second
   model, a proxy, or a local model endpoint.
6. [Specialists](./specialists) — read this when you want to change the role
   binding on a column or a session.

## What this section covers

- what each feature is for, in user-experience terms (not internals)
- the shortest happy path through each one
- when to pick one feature over another
- a small set of patterns that work in real projects

If you are looking for the internal model (data shapes, SSE wire format,
provider trait, tool registry), read `docs/ARCHITECTURE.md` and the per-topic
docs under `docs/`. This section is the user-facing counterpart.

## Read next

- [Quick start](./quickstart) — same five-minute path, in checklist form
- [Workspaces](./workspaces)
- [Sessions](./sessions)
- [Kanban](./kanban)
- [Common workflows](./common-workflows)
- [Best practices](./best-practices)
