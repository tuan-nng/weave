-- Migration 017: kanban-auto-spawned session lifecycle supervision.
--
-- Adds the `kanban_session_watch` table that the lifecycle supervisor
-- (feat-067) reads to detect stalled sessions. One row per
-- kanban-auto-spawned session, with `last_activity_at` bumped on every
-- SSE event for the session and on every `send_prompt` call. The
-- supervisor scans the table every 30s and recovers sessions whose
-- `last_activity_at` is older than the column's `stall_threshold_seconds`
-- (default 300s).
--
-- Cascade FK to `sessions(id)`: if a session is deleted, its watch row
-- is removed automatically. The `task_id` is informational — it tells
-- the operator which card the supervisor was watching without joining
-- the tasks table.

CREATE TABLE IF NOT EXISTS kanban_session_watch (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL,
    last_activity_at TEXT NOT NULL,
    recovery_count INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'watching'
);

CREATE INDEX IF NOT EXISTS idx_kanban_session_watch_status_activity
    ON kanban_session_watch(status, last_activity_at);
