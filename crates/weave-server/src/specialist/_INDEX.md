# specialist/ — Specialist YAML Loading

Loads specialist definitions from YAML files on disk. Specialists are pre-configured agent personas with system prompts, tool profiles, and model settings. Single file: `mod.rs` (13KB, ~434 lines).

## Key Types

| Type | Purpose |
|------|---------|
| `SpecialistRegistry` | Loads and holds all specialist definitions in memory. Created at startup. |
| `Specialist` | A specialist definition: `id`, `name`, `description`, `system_prompt` (Markdown), `tool_profile` (profile name string), `default_model`, `temperature` |

## Public API

- `SpecialistRegistry::load_from_dir(path)` — scans directory for `*.yaml` files, parses YAML frontmatter
- `list(&self)` — returns all loaded specialists
- `get_by_id(&self, id)` — lookup by ID

## YAML Format

Each specialist file uses YAML frontmatter:
```yaml
id: code-reviewer
name: Code Reviewer
description: Reviews code for bugs and quality issues
tool_profile: full
default_model: claude-sonnet-4-6
temperature: 0.3
---
System prompt in Markdown...
```

## Built-in Specialists (5)

Shipped in `resources/specialists/`:
- `general-purpose` — default agent, full tool profile
- `code-reviewer` — code review specialist
- `tdd-guide` — test-driven development coach
- `architect` — system design consultant
- `bug-hunter` — debugging specialist

## Connections

- **Used by:** `api/specialists.rs` (list endpoint), `service/sessions.rs` (system prompt injection), `service/kanban.rs` (auto-trigger specialist selection)
- **No dependencies** on other weave modules — loads from filesystem only
