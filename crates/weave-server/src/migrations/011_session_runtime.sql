-- Migration 011: Per-session runtime / mode / runtime metadata.
--
-- The multi-runtime strategy (Phase 7) requires each session row to
-- record which Runtime Tool it uses, which transport mode the agent
-- runs in, and a per-runtime JSON blob (e.g. the CLI-native session
-- id used to resume a Claude Code turn on a subsequent prompt).
--
-- New columns (all on `sessions`):
--   * `runtime_kind`           TEXT NOT NULL DEFAULT 'anthropic-api'
--       One of: anthropic-api | openai-api | openai-compatible |
--               claude-code     | codex       | opencode
--   * `mode`                   TEXT NOT NULL DEFAULT 'native'
--       One of: native | wrapped | attended
--   * `runtime_metadata_json`  TEXT
--       Nullable JSON blob whose shape is keyed on `runtime_kind`.
--       For CLI runtimes the canonical field is `cli_resume_id` (the
--       session id the wrapped subprocess returned on its previous
--       turn). The column is nullable because the HTTP/native runtimes
--       have no per-session state to carry.
--
-- Existing rows backfill via the column-level DEFAULTs — every
-- pre-migration session is treated as an `anthropic-api` `native`
-- session with no metadata, which is exactly the pre-feat-038
-- behavior.
--
-- No CHECK constraints: enum validation is Rust-only (mirrors the
-- existing `status` and `provider.type` precedent — keeps the DB
-- migration cheap and side-steps future enum-evolution churn).
-- No index on the new columns: the multi-runtime work that will
-- filter by `runtime_kind` (Phase 9: provider UIs) is the right place
-- to add the index when the query plan needs it, not this migration.

ALTER TABLE sessions
    ADD COLUMN runtime_kind TEXT NOT NULL DEFAULT 'anthropic-api';

ALTER TABLE sessions
    ADD COLUMN mode TEXT NOT NULL DEFAULT 'native';

ALTER TABLE sessions
    ADD COLUMN runtime_metadata_json TEXT;
