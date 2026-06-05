-- Migration 010: Add codebase_id to sessions for codebase-bound sessions.
--
-- A session may be bound to a registered codebase (git repo). When bound:
--   * `codebase_id` references `codebases(id)` and is non-null.
--   * The session's `cwd` is set to the codebase's `path` at create time.
--   * The tool context's `codebase_root` resolves via `find_by_cwd_prefix`
--     so fs_write/fs_edit sandbox to the codebase.
--
-- On codebase delete, `codebase_id` is set to NULL (ON DELETE SET NULL).
-- The session row, its messages, and the previously-stored `cwd` survive
-- the delete — the session just becomes unbound. Re-binding requires
-- creating a new session.
--
-- Existing sessions: `codebase_id` is NULL, so no backfill is required.

ALTER TABLE sessions ADD COLUMN codebase_id TEXT
    REFERENCES codebases(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_sessions_codebase_id ON sessions(codebase_id);
