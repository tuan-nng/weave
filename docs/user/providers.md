# Providers

A provider is a configured model backend — the thing that actually answers
the prompts you send in a session. Weave ships with one provider type
(`anthropic`); any OpenAI-compatible endpoint works behind it by changing
the base URL.

## What providers are for

- holding the credentials needed to call a model API
- choosing which model a session uses by default
- letting you run side-by-side (Anthropic direct + a local proxy + a
  cheaper model for triage) without rewriting prompts

A provider is **not** a specialist and **not** a session. A session picks
one provider at creation time and stays on it for the rest of its life.

## How to use it

### Add a provider

1. Click `Settings` in the left sidebar.
2. In the `Add Provider` form, fill in:
   - **Name** — anything you recognise in the dropdown later
     (`Production`, `Local Qwen`, `Staging`).
   - **Type** — read-only in v1, fixed at `Anthropic`. The text input shows
     `Anthropic` and is greyed out.
   - **Base URL** — defaults to `https://api.anthropic.com`. Change this if
     you are pointing at a proxy or a local model server that speaks the
     Anthropic wire format.
   - **API Key** — paste your key. It is stored server-side and never echoed
     back in the UI.
   - **Default Model** — the model used when a session is created without an
     explicit `model` override. Anything that the target API accepts
     (`claude-sonnet-4-20250514`, `claude-opus-4-5`, etc.).
3. Click `Add Provider`. It appears in the `Providers` table below.

### Use a provider

You never use a provider directly. It shows up in two places:

- the **Provider** dropdown in the `+ New Session` modal
- the **Provider** field when a Kanban column auto-triggers a session

Pick one at session creation. The session keeps that provider for every
turn, including any sub-agents the specialists spin up.

### Delete a provider

1. Hover the row in the `Providers` table. A `Delete` button fades in.
2. Click it, then confirm.

Deleting a provider does **not** retroactively change sessions that already
used it. They keep working (or failing) against the configuration they were
created with. If you delete the only provider, the `+ New Session` modal's
dropdown will be empty and you cannot create new sessions until you add one
back.

## Choosing between providers

| Use case | Pick |
| --- | --- |
| Production work, highest quality | Anthropic direct with `claude-opus-4-5` or `claude-sonnet-4-20250514` |
| Cheaper triage of many small tasks | Anthropic direct with `claude-haiku-4-5-20251001` |
| Air-gapped / no external calls | Local model endpoint via proxy, with its own base URL and key |
| Audit / staging | A second provider with the same model but a different key, so you can compare |

## Health and reachability

The binary's health endpoint reports per-provider health:

```
GET /api/health
```

returns `providers: { total, healthy, unhealthy }`. The UI does not surface
this yet, so if a session is failing on startup, check that endpoint first
to see whether the provider is the problem.

## Common pitfalls

**"I added the provider but the dropdown is empty in `+ New Session`."**
The session creation query is cached. Refresh the page once. If it is still
empty, the create-provider call probably failed and only the form cleared —
check the error banner at the top of `Settings`.

**"Sessions are failing with a 401."** The API key was rotated, or the
base URL no longer matches the API the key is for. Update the provider's
key/URL in `Settings` and the next session will pick it up.

**"I want a different model for one session."** Open `+ New Session` and
fill the optional `Model` field with the model ID. Leaving it empty uses
the provider's default.

**"I want to run my own model server."** Point the provider's base URL at
your server and make sure it speaks the Anthropic streaming format (or wrap
it in a proxy that does). Weave does the rest of the wire format for you.

## Read next

- [Sessions](./sessions) — where the provider actually gets used
- [Workspaces](./workspaces) — providers are global; workspaces are not
- [Common workflows](./common-workflows) — when to use a second provider
