-- Migration 006: Per-column transition gate policies.
--
-- Adds three per-column fields that constrain what tasks can do
-- when they leave (freeze_description) or enter (required_fields,
-- required_artifact_types) a column.
--
-- Design notes:
--   - freeze_description: boolean, same shape as auto_trigger.
--     When true, tasks in this column must already have a non-empty
--     description before they can be moved out. Enforced in
--     service::kanban::check_transition_gates (the move path).
--   - required_fields: JSON array of tasks field names. Valid values
--     are: acceptance_criteria | completion_summary | verification_report.
--     Tasks moving INTO this column must have non-empty values for
--     each named field. Enforced in service::kanban::check_transition_gates.
--   - required_artifact_types: JSON array of artifact type strings
--     (free vocabulary — feat-031 owns the type registry). The gate
--     check is a stub in feat-028: the schema ships now, the
--     enforcement is acknowledged-only until feat-031 lands.
--
-- Defaults are empty/no-op so existing columns (and the default
-- 5-column template from feat-027) are unaffected. Boards opt in
-- to gate policies by PATCHing a column's freeze_description /
-- required_fields / required_artifact_types fields.
--
-- SQLite ALTER TABLE ADD COLUMN is non-destructive: existing rows
-- get the DEFAULT value, no table-recreate dance needed (unlike
-- migration 005 which had to change an FK in place).

ALTER TABLE columns ADD COLUMN freeze_description INTEGER NOT NULL DEFAULT 0;
ALTER TABLE columns ADD COLUMN required_fields TEXT NOT NULL DEFAULT '[]';
ALTER TABLE columns ADD COLUMN required_artifact_types TEXT NOT NULL DEFAULT '[]';
