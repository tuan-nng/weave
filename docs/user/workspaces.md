# Workspaces

A workspace is the top-level container for everything else in Weave. Sessions,
boards, and codebases all live inside one workspace. Providers are the only
thing that lives outside — they are global, because the same Anthropic account
can serve every workspace.

## What workspaces are for

- keeping unrelated projects from sharing state (sessions, codebases, boards)
- giving each project a clean name and a place to start
- scoping what a single set of specialists and boards applies to

You usually want **one workspace per project or per team**. You do not want
one workspace per session — that fragments the UI and loses the benefit of
the workspace boundary.

## How to use it

### Open the Workspaces list

Click `Home` in the left sidebar. The page is called "Workspaces" and shows
every workspace the binary knows about, newest first.

### Create a workspace

1. In the `Enter workspace name...` field at the top, type a name. It must
   be unique (case-insensitive).
2. Click `Create`. The new workspace appears in the list immediately. It is
   not auto-selected — click the name to open it.

### Open a workspace

Click the workspace name. You land on the workspace overview, which shows a
session table (empty at first) and a `+ New Session` button.

### Rename a workspace

1. Hover the row. Two buttons fade in: `Rename` and `Delete`.
2. Click `Rename`. The name becomes an inline input.
3. Type the new name and press `Enter`. `Escape` cancels.

### Delete a workspace

1. Hover the row, click `Delete`. A confirm modal appears.
2. Confirm. **All sessions, boards, and codebases in the workspace are
   removed.** The binary does not ask per-resource.

The `★ Default` workspace cannot be renamed or deleted. It exists so the
binary always has at least one workspace to route new work to. To "remove"
it, create a replacement workspace and use that instead.

## When to add a new workspace

Add a new workspace when:

- you are switching to a project that has nothing to do with the current one
- you want a clean session history without losing the old one
- multiple people are sharing one Weave install and need hard boundaries

Do not add a new workspace when:

- you are just starting a new task in the same project — open a new session
  in the existing workspace instead
- you want to keep two "views" of the same project — the UI does not have
  that concept, and creating parallel workspaces will silently fragment
  the work

## Common pitfalls

**"I deleted a workspace by accident."** The confirm modal says "All
sessions will be removed." This is the only place Weave warns you. If you
need soft-delete or a recycle bin, the binary does not have one.

**"I have five workspaces called 'Untitled'."** Weave does not enforce a
non-empty default. Use the rename flow on each one. Future-you will thank
present-you.

**"My new workspace shows '0 sessions' but I had sessions earlier."**
Sessions are workspace-scoped. They did not migrate; they belong to the
workspace you created them in. Open the old workspace from `Home` to see
them.

## Read next

- [Sessions](./sessions) — what to do once a workspace is open
- [Codebases](./codebases) — register a git repo inside the workspace
- [Kanban](./kanban) — add a board to the workspace
- [Best practices](./best-practices) — how to think about workspace
  boundaries
