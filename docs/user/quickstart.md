# Quick Start

The right goal for your first five minutes is not "understand all of Weave".
It is one successful round trip: configure a provider, ask the agent to look
at a real path, and read its answer.

## Before you start

- The Weave binary is running and reachable in a browser (default
  `http://localhost:3000`).
- You have an Anthropic API key, an OpenAI-compatible proxy, or a local model
  endpoint ready.

## First success checklist

1. Add one provider in `Settings`.
2. Open the default workspace from `Home`.
3. Create a new session against that provider.
4. Send one concrete prompt about a real path or file.
5. Open the Journey sidebar and confirm files were read.

## Step 1 — Add a provider

1. Click `Settings` in the left sidebar.
2. Fill in the `Add Provider` form:
   - **Name** — anything you recognise (e.g. `Production`, `Local Qwen`,
     `Staging`).
   - **Type** — fixed at `Anthropic` in v1. Any OpenAI-compatible endpoint
     works by overriding the **Base URL** below.
   - **Base URL** — `https://api.anthropic.com` for direct Anthropic, or
     your proxy URL.
   - **API Key** — paste your key. It is stored server-side and never echoed
     back to the UI.
   - **Default Model** — e.g. `claude-sonnet-4-20250514`.
3. Click `Add Provider`. It appears in the `Providers` table at the bottom of
   the page.

If `Settings` shows `Failed to load providers` or the form errors on submit,
the binary is up but the database is unhealthy — go to
`http://localhost:3000/api/health` and read the `database.reachable` field
before continuing.

## Step 2 — Open a workspace

1. Click `Home` in the left sidebar. You land on the Workspaces list.
2. The first row is `★ Default` and cannot be renamed or deleted. Click it to
   open the workspace.
3. The workspace overview shows session stats: total, active, completed,
   errors. Empty for a brand-new install — that is expected.

## Step 3 — Create your first session

1. From the workspace page, click `+ New Session`.
2. In the modal:
   - **Provider** — pick the one you just added.
   - **Specialist** — leave empty for the first run.
   - **Model** — leave empty to use the provider's default.
3. Click `Create Session`. The chat page opens automatically.

## Step 4 — Send a real prompt

Type a prompt that requires the agent to actually do work, not a vague
question. Good smoke tests:

- *"List the top-level files in `/path/to/your/repo` and explain what each
  one is for."*
- *"Read `Cargo.toml` and tell me which dependencies look unused."*
- *"Run `git log --oneline -10` in `/path/to/your/repo` and summarise the
  last week of changes."*

While the agent is working, you will see:

- streaming text appearing token-by-token in the main column;
- a tool-call block per tool the agent invokes (`fs_read`, `shell_exec`,
  `git_log`, etc.), with the input, output, and duration;
- a "Stop" button at the top of the page. Click it to cancel mid-flight.

## Step 5 — Read the Journey sidebar

1. On the right edge of the session page is a thin rail with a chart icon.
   Click it to open the Journey sidebar.
2. **Decisions & Errors** lists, top-to-bottom, what the agent decided and
   any errors it hit.
3. **Files** deduplicates everything the agent touched. Click a path to copy
   it.

If `Files` is empty, the agent answered from its own knowledge and never
opened a file. That is fine for a trivial prompt, but for any real task you
want at least one `read` in there.

## What to read next

- [Sessions](./sessions) — full tour of the chat surface
- [Journey sidebar](./journey) — how to read what the agent actually did
- [Workspaces](./workspaces) — when to add a second workspace
- [Common workflows](./common-workflows) — patterns that work in real
  projects
