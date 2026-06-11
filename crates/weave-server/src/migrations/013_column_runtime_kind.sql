-- Migration 013: Per-column runtime_kind binding for kanban lane automation.
--
-- Adds a nullable `runtime_kind` column to `columns` so each column
-- can specify which Runtime Tool (e.g. "claude-code", "anthropic-api")
-- should be used when lane automation spawns a session. NULL means
-- "inherit from the workspace's first provider" (the pre-feat-055
-- default behavior).

ALTER TABLE columns ADD COLUMN runtime_kind TEXT;
