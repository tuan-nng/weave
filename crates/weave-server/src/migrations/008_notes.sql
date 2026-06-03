-- Migration 008: Workspace notes (feat-030).
--
-- A note is a workspace-scoped piece of long-form text an agent or
-- human keeps between sessions: a spec, a task description, a meeting
-- summary, a design rationale, etc. Notes are not tied to a single
-- task — they live alongside the workspace so any agent that joins
-- the workspace can read them.
--
-- Design notes:
--   - `type` is a strict whitelist of three values: `spec`, `task`,
--     `general`. Unlike artifacts (where the type is a free
--     vocabulary the column gate names), notes are categorized at
--     write time. The store's `validate_note_type` enforces the
--     whitelist; the DB column itself is plain TEXT to keep the
--     schema out of the way of future expansion.
--   - UNIQUE (workspace_id, title) is the title-scope constraint: no
--     two notes in the same workspace can share a title. This powers
--     `create_note`'s unique-violation detection (mapped to
--     `Conflict` by `db::map_insert_error`) and `read_note`'s lookup
--     pattern.
--   - A second index on `workspace_id` alone is included for
--     `list_notes`'s workspace-scoped filter. SQLite would also be
--     able to use the composite UNIQUE index's leading column, but a
--     dedicated index keeps the planner's choice obvious and the
--     `list_notes` EXPLAIN QUERY PLAN predictable.
--   - workspace_id is ON DELETE CASCADE: deleting a workspace
--     carries its notes. The store does not expose a delete method
--     — the spec deliberately omits it, so the only way a note
--     leaves a workspace is if the workspace itself does.
--   - content is NOT NULL with empty string default. The two
--     write-style tools (`create_note` and `append_to_note`) can
--     both produce notes with no content yet, which `set_note_content`
--     later fills. Matches the artifact precedent (migration 007).
--   - The `IF NOT EXISTS` clauses make the migration idempotent so
--     `test_migrations_idempotent` (db.rs:184) continues to pass when
--     a fresh DB has migration 008 applied twice during a runner
--     reset. Same pattern as migrations 006 and 007.

CREATE TABLE IF NOT EXISTS notes (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    title        TEXT NOT NULL,
    type         TEXT NOT NULL,
    content      TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_workspace_title ON notes(workspace_id, title);
CREATE        INDEX IF NOT EXISTS idx_notes_workspace       ON notes(workspace_id);
