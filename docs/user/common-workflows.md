# Common Workflows

These are the patterns that come up most often in real projects. None of
them are required — Weave works as a chat if you want it to — but they
are the shapes that pay off.

## 1. Understand a new repository

Use this when you are entering an unfamiliar codebase and want the
fastest route to a useful mental model.

1. Register the repo as a [Codebase](./codebases) in your workspace.
2. Open a [Session](./sessions) with no specialist.
3. Ask for a high-level overview first: *"Read `README.md` and the top
   level of `/path/to/repo`. Give me a 5-bullet description of what this
   project is for."*
4. Follow up with sharper questions: *"Where does HTTP routing live?",
   "What's the data model for X?", "How is auth wired up?"*
5. Open the [Journey](./journey) sidebar after each turn. If `Files` is
   empty, the agent is guessing — re-prompt and ask it to read the
   specific files first.

**Best for:** orientation, scoping work before changing code, building
a mental model of a repo you have never touched.

## 2. Implement one concrete task

Use this when the task is clear and does not need workflow stages yet.

1. Open a workspace and confirm a [Provider](./providers) is configured.
2. Create a [Session](./sessions) with the `dev-crafter` specialist.
3. Send one concrete implementation request: *"Add a `health` endpoint
   to the API that returns the database size in bytes. Update
   `routes/health.rs` and add a test."*
4. Watch the [Journey](./journey) sidebar while the agent works. The
   `Files` section tells you whether it actually edited the file you
   expected.
5. When the agent finishes, open a second session (or send another
   prompt) with the `review-guard` specialist: *"Run the tests and
   review the diff in `/path/to/repo`."*

**Best for:** feature work, bug fixes, docs updates, short refactors.

## 3. Move work through delivery stages

Use this when the work is repeatable and benefits from explicit gates.

1. Open the workspace's [Kanban](./kanban) board (or create one).
2. Seed the columns. The default five-stage shape (`Backlog`, `To Do`,
   `In Progress`, `Review`, `Done`) is a good starting point.
3. Bind each column to a [Specialist](./specialists) and turn
   `Auto-trigger` on for the first four.
4. Drop a card in `Backlog`. The `backlog-refiner` session runs and
   rewrites the card's `description` and `acceptance_criteria`.
5. Drag the card to `To Do`. The `todo-orchestrator` session runs and
   plans the execution.
6. Drag the card to `In Progress`. The `dev-crafter` session runs and
   implements.
7. Drag the card to `Review`. The `review-guard` session runs and
   either writes a `verification_report` (approved) or pushes the card
   back to `In Progress` with comments (rejected).
8. Drag approved cards to `Done`. The `done-reporter` is **not**
   auto-triggered; you run it manually when you want a summary.

**Best for:** multi-task work that follows the same shape, team
visibility into progress, and review gates that actually block.

## 4. Recover from a bad run

Use this when a session has gone off the rails and you want to start
cleanly without losing the history.

1. Open the bad session.
2. Click `Stop` if it is still running.
3. Read the [Journey](./journey) sidebar to understand what went wrong.
4. Open a new session. Pass the old `session_id` as
   `parent_session_id` if you want the new session to inherit the
   prior turns as context, or leave it empty for a clean slate.
5. Rephrase the prompt with what you learned from the journey.

**Best for:** debugging agent behaviour, salvaging partially-correct
runs, iterative prompt refinement.

## 5. Run the same flow against a new repo

Use this when you have a working Kanban flow in one workspace and want
to reuse it in another (e.g. a new microservice, a new client's
codebase).

1. Create a new workspace.
2. Register the new [Codebase](./codebases) pointing at the new repo.
3. Create a new board with the same column shape and the same
   specialist bindings.
4. Drop a card. The same auto-trigger flow runs against the new repo.

The specialists and the column structure are *your* configuration.
They do not move with code or with sessions. Set them up once per
workspace.

**Best for:** productising an agent workflow, onboarding a new project
quickly.

## 6. Audit a finished run

Use this when a session is done and you want to know whether to trust
it.

1. Open the session.
2. Read the chat from top to bottom. The agent's final message is
   usually a summary of what it did.
3. Open the [Journey](./journey) sidebar. Look for:
   - a non-empty `Files` section (the agent actually touched code);
   - decision rows that match the work in the chat (the agent had a
     reason for each step);
   - no error rows, or error rows that are followed by recovery.
4. For a Kanban-bound card, open the card detail panel and read the
   `verification_report` written by the review specialist.

**Best for:** deciding whether to merge the agent's work, understanding
a session you did not run, post-mortem on a bad result.

## 7. Switch models mid-project

Use this when you realise the current provider's default model is too
slow / too expensive / too weak for the work in this workspace.

1. Open `Settings`.
2. Add a second provider with the new model and a distinct name (e.g.
   `Production-Sonnet`, `Local-Haiku`).
3. Existing sessions keep using the provider they were created with.
   New sessions can pick the new one in the `+ New Session` modal.
4. If the switch is workspace-wide, delete the old provider *after*
   all in-flight sessions have completed. Deleting a provider mid-run
   does not break the session, but new sessions in the workspace can
   no longer pick it.

**Best for:** cost tuning, performance troubleshooting, A/B testing two
models on the same task.

## Read next

- [Best practices](./best-practices) — short rules to keep sessions and
  boards from getting out of hand
- [Sessions](./sessions) — the chat surface every workflow uses
- [Kanban](./kanban) — the delivery flow most workflows are built on
