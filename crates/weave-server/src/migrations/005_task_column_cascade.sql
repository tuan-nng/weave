-- Migration 005: ON DELETE CASCADE on tasks.column_id
--
-- Migration 002 declared the FK without CASCADE, so deleting a column
-- with tasks in it would fail with a FK violation. The original choice
-- was conservative; feat-024 wants cascade so column cleanup is atomic
-- with its child tasks (a deleted column carries its cards with it).
--
-- SQLite cannot ALTER an existing foreign key in place. The standard
-- pattern is to disable FK enforcement, recreate the table with the
-- new constraint, copy the data, drop the old table, and rename.
-- The whole sequence is wrapped in a transaction so it commits
-- atomically from the perspective of any concurrent reader.

PRAGMA foreign_keys = OFF;
BEGIN TRANSACTION;

CREATE TABLE tasks_new (
    id          TEXT PRIMARY KEY,
    board_id    TEXT NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
    column_id   TEXT NOT NULL REFERENCES columns(id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    description TEXT,
    position    INTEGER NOT NULL DEFAULT 0,
    status      TEXT NOT NULL DEFAULT 'active',
    session_id  TEXT REFERENCES sessions(id),
    acceptance_criteria TEXT,
    completion_summary  TEXT,
    verification_report TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

INSERT INTO tasks_new (
    id, board_id, column_id, title, description, position, status,
    session_id, acceptance_criteria, completion_summary, verification_report,
    created_at, updated_at
)
SELECT
    id, board_id, column_id, title, description, position, status,
    session_id, acceptance_criteria, completion_summary, verification_report,
    created_at, updated_at
FROM tasks;

DROP TABLE tasks;
ALTER TABLE tasks_new RENAME TO tasks;

CREATE INDEX idx_tasks_board  ON tasks(board_id);
CREATE INDEX idx_tasks_column ON tasks(column_id);

COMMIT;
PRAGMA foreign_keys = ON;
