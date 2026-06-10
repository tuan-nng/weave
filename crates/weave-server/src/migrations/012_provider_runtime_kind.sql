-- Migration 012: Per-provider runtime kind + CLI fields.
--
-- feat-039 widens the providers table from "one HTTP shape" to a
-- discriminated union on `kind` (http | cli). The HTTP shape is the
-- pre-existing path; the CLI shape lets us pre-register Claude Code /
-- Codex / OpenCode providers before their dispatch adapters land in
-- feat-051.
--
-- New columns (all on `providers`):
--   * `kind`             TEXT NOT NULL DEFAULT 'http'
--       One of: http | cli
--   * `default_model`    TEXT
--       Canonical wire field for both kinds. Backfilled from the
--       pre-existing `config_json.default_model` key for HTTP rows.
--   * `binary_path`      TEXT
--       Filesystem path to the CLI binary (cli rows only).
--   * `args_json`        TEXT
--       JSON-encoded Vec<String> of CLI args (cli rows only).
--   * `env_json`         TEXT
--       JSON-encoded BTreeMap<String, String> of CLI env vars (cli rows
--       only).
--   * `permission_mode`  TEXT
--       Opaque permission-mode string (cli rows only). The closed enum
--       arrives in feat-046.
--
-- Existing rows backfill to `kind='http'` via the column-level DEFAULT;
-- `default_model` is extracted from the prior `config_json` blob when
-- present. The `config_json` column is kept (it still carries `api_key`
-- for HTTP rows; for CLI rows it holds a one-key `{"default_model": ...}`
-- wrapper per the locked-in decision to keep `service/sessions.rs:318`
-- `default_model` extraction working for both kinds).
--
-- No CHECK constraints: enum validation is Rust-only (mirrors the
-- existing `status`, `provider.type`, and `session.runtime_kind`
-- precedent — keeps the migration cheap and side-steps future
-- enum-evolution churn).
-- No index on the new columns: the work that will filter by `kind`
-- (Phase 9: provider UIs) is the right place to add an index when the
-- query plan needs it, not this migration.

ALTER TABLE providers
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'http';

ALTER TABLE providers
    ADD COLUMN default_model TEXT;

ALTER TABLE providers
    ADD COLUMN binary_path TEXT;

ALTER TABLE providers
    ADD COLUMN args_json TEXT;

ALTER TABLE providers
    ADD COLUMN env_json TEXT;

ALTER TABLE providers
    ADD COLUMN permission_mode TEXT;

-- Backfill: copy any pre-existing default_model from config_json.
-- Idempotent: the WHERE clause is a no-op on re-run because
-- `default_model IS NULL` is false after the first apply.
UPDATE providers
   SET default_model = json_extract(config_json, '$.default_model')
 WHERE default_model IS NULL
   AND json_extract(config_json, '$.default_model') IS NOT NULL;
