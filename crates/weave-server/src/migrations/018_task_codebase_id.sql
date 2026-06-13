-- Migration 018: card-level codebase binding (feat-068).
--
-- Adds `tasks.codebase_id` (nullable TEXT) so a card can pin a
-- specific codebase for sessions spawned by lane automation. The
-- column is nullable: `NULL` preserves the pre-feat-068 behavior
-- (try_automate_lane picks the workspace's first registered
-- codebase for CLI runtimes, None for HTTP runtimes).
--
-- No foreign key on `codebase_id` because the validator
-- (try_automate_lane + the column-level binding in feat-050's
-- `validate_wrapped_session_cwd`) does the lookup at session
-- creation time. A stale codebase_id (codebase deleted) surfaces
-- as a clear 4xx with `code: "cwd_outside_codebase"` rather than
-- a hidden FK violation, which is the better error surface for the
-- operator.

ALTER TABLE tasks ADD COLUMN codebase_id TEXT;
