-- Migration 007: Task artifacts (feat-031).
--
-- An artifact is a piece of evidence an agent attaches to a kanban
-- task: a test result dump, a screenshot URL, a code diff, a log
-- snippet, etc. Columns can require specific artifact types before
-- a task is allowed to transition into them (see the
-- `required_artifact_types` JSON column added in migration 006; the
-- gate check is filled in by feat-031).
--
-- Design notes:
--   - `type` is a free-vocabulary TEXT, not an enum. Columns name the
--     types they require, agents decide what to attach. Keeps the
--     schema out of the way of new use cases (traces, perf numbers,
--     ...).
--   - UNIQUE (task_id, type) is the single index that powers BOTH
--     `provide_artifact`'s upsert (`INSERT ... ON CONFLICT ... DO
--     UPDATE`) AND `list_by_task`'s WHERE task_id=? filter. No
--     separate `task_id` index needed.
--   - task_id is ON DELETE CASCADE: deleting a task (or a column that
--     cascades, or a board) carries its artifacts. The store does
--     not expose a delete method.
--   - content is NOT NULL with empty string default. `request_artifact`
--     creates a row whose content is the empty string; the agent
--     later fills it via `provide_artifact` (which upserts).
--   - The `IF NOT EXISTS` clause makes the migration idempotent so
--     `test_migrations_idempotent` (db.rs:184) continues to pass when
--     a fresh DB has migration 007 applied twice during a runner
--     reset. Same pattern as `ALTER TABLE ... DEFAULT` statements
--     in migration 006.

CREATE TABLE IF NOT EXISTS artifacts (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    type        TEXT NOT NULL,
    content     TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_artifacts_task_type ON artifacts(task_id, type);
