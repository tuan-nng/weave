# Codebases

A codebase is a git repository registered against a workspace. It is the
thing that gives a session a real working directory to read from, run
shell commands in, and commit to.

## What it is for

- giving the agent a fixed path it can read, edit, and run commands
  against
- surfacing the current branch and recent commit history inside Weave
- scoping which git repo a session's "list the files" prompt is allowed
  to mean

A workspace with no codebases is still a valid workspace, but every
session in it is answering prompts from the model alone, with no local
files to ground its answers.

## How to use it

### Open the Codebases list

Click `Codebases` in the left sidebar. The page lists every registered
codebase, grouped by workspace.

### Register a codebase

1. From the `Codebases` list, click `+ New codebase in <workspace>`.
2. In the modal, enter:
   - **Path** — absolute path to the git repository on the Weave host.
     The binary runs on the same machine you are running it from, so this
     is a local path, not a URL.
   - **Branch** — optional. Defaults to the repo's current branch at
     registration time.
   - **Label** — optional friendly name. If you leave it blank, the path
     is used as the display name.
3. Click `Register`. Weave runs `git status` once to confirm the path is
   actually a repo. If it is not, the request fails with a `git_error`
   surfaced as a banner on the codebase detail page.

### Read a codebase

Click a codebase row. The detail page has three sections:

1. **Codebase** — the row identity: full path, label, branch, workspace
   ID prefix, creation time.
2. **Git status** — current branch, and a list of dirty files (modified,
   added, untracked). Clean repos show `clean` next to "Dirty files".
3. **Recent commits** — the last 10 commits with short SHA and message.

If the path is not a git repo, or git is not installed, only the
`Codebase` section renders; the rest is suppressed in favour of an error
banner.

### Delete a codebase

1. On the codebase detail page, click `Delete` in the header.
2. Confirm in the browser dialog.

This removes the registration from the workspace. It does **not** touch
the directory on disk — your git repo is unchanged.

## How sessions use a codebase

Sessions can be **bound** to a codebase at creation time. The `New
Session` modal has a `Codebase` dropdown listing every registered
codebase in the workspace. When you pick one:

- the session's working directory (`cwd`) is set to the codebase's
  `path`;
- the session's `codebase_id` is recorded, so the binding survives
  across the lifetime of the session;
- the session page header shows the codebase's basename, with the
  full path on hover;
- the agent's `fs_read`/`fs_list`/`fs_search` and the explicit-`cwd`
  form of `shell_exec`/`git_*` are contained: any request to operate
  outside the repo root returns a clear error.

The sandbox is on **the working-directory argument, not the shell
command body**. `fs_read {path}` and `shell_exec {cwd}` cannot escape
the repo, but the shell command itself (after `sh -c`) can do
anything the server process can — including `cat /etc/passwd` or
`ln -s /etc <repo>/etc_link`. Symlinks inside the codebase are
deliberately not followed by the FS walkers (`fs_list`, `fs_search`),
so the second trick does not work either. Think of the binding as a
permission boundary on the cwd, not as a jail on the shell.

If you don't pick a codebase, the session starts unbound: `cwd` falls
back to the workspace root, and the FS tools stay permissive (they
can read any absolute path the server can reach). Picking a codebase
is the explicit opt-in to sandboxing.

Kanban auto-spawned sessions do not yet pick a codebase — the
`tasks` model has no `codebase_id`, and the auto-spawn falls back to
the legacy "operate in the workspace root" behavior. Re-binding
requires a new session.

If a codebase is deleted while sessions are still bound to it, the
binding is broken (`codebase_id` becomes `null`) but the sessions
survive. The runtime falls back to the session's stored `cwd` as
the containment root, so the sandbox stays active.

## When to register a codebase

Register when:

- the agent will need to read or edit real files (the common case);
- you want git status and recent commits visible in Weave as a quick
  sanity check;
- you want a session's tool calls to have a real `cwd` to operate in.

Do not register when:

- the work is purely conversational (planning, review of a description,
  Q&A);
- the directory is huge and you only want to point the agent at a
  subdirectory — pass the subdirectory path in the prompt instead.

## Common pitfalls

**"The detail page shows `git_error: not a git repository`."** You
registered a path that is not a git repo, or it is a bare clone, or git
is not installed on the Weave host. Fix the path or the host, then
delete and re-register the codebase.

**"Dirty files shows a path I did not touch."** The agent ran in this
working directory and modified files. Open the session that ran most
recently, check the [Journey sidebar](./journey), and either commit the
changes (so the working tree is clean) or revert them with `git
checkout`.

**"I registered a codebase but my session does not see it."** Sessions
no longer auto-bind to codebases. Pick the codebase from the
`Codebase` dropdown in the `New Session` modal, or pass its path in
the prompt.

**"Branch is wrong."** The branch is captured at registration time and
is not auto-refreshed. Delete and re-register the codebase against the
new branch.

## Read next

- [Sessions](./sessions) — how sessions and codebases interact
- [Kanban](./kanban) — boards can sit alongside codebases in the same
  workspace
- [Common workflows](./common-workflows) — typical workspace + codebase
  pairings
