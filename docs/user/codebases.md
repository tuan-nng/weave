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

Sessions do not auto-bind to a codebase. The session page does not show
"current codebase" anywhere. What actually happens is:

- the session's working directory defaults to the workspace root;
- if a session is auto-triggered from a Kanban card, the card's
  `description` and `acceptance_criteria` are what the agent reads to
  decide what to do, and the agent figures out the relevant path from
  tool calls (`fs_read`, `shell_exec pwd`, `git log`).

If you want the agent to operate on a specific repo, your prompt should
name the path. *"Read `/home/me/projects/weave/Cargo.toml` and..."* is
unambiguous; *"Read the project config"* is not, if there is more than
one codebase registered.

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
do not bind to codebases automatically. Pass the path in the prompt, or
set the session's `cwd` to the codebase root.

**"Branch is wrong."** The branch is captured at registration time and
is not auto-refreshed. Delete and re-register the codebase against the
new branch.

## Read next

- [Sessions](./sessions) — how sessions and codebases interact
- [Kanban](./kanban) — boards can sit alongside codebases in the same
  workspace
- [Common workflows](./common-workflows) — typical workspace + codebase
  pairings
