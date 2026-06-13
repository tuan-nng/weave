-- Migration 016: Column automation config + validation cache.
--
-- Adds `automation_json` TEXT to columns for per-column gate configuration
-- (delivery, contract, checklist, validator rules + gate_mode toggle).
-- NULL = no automation (legacy behavior, backward-compatible).
--
-- Adds `kanban_validations` table to cache validator command results,
-- keyed by (task_id, command_key). Prevents re-running subprocess on
-- every move attempt.

ALTER TABLE columns ADD COLUMN automation_json TEXT;

CREATE TABLE IF NOT EXISTS kanban_validations (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    command_key TEXT NOT NULL,
    result INTEGER NOT NULL,
    cached_at TEXT NOT NULL,
    UNIQUE(task_id, command_key)
);

CREATE INDEX IF NOT EXISTS idx_kanban_validations_task
    ON kanban_validations(task_id);
