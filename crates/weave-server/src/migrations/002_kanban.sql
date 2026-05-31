-- Migration 002: Kanban and trace tables
-- boards, columns, tasks, traces, file_changes

-- Kanban: Boards
CREATE TABLE boards (
    id              TEXT PRIMARY KEY,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

-- Kanban: Columns
CREATE TABLE columns (
    id              TEXT PRIMARY KEY,
    board_id        TEXT NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    position        INTEGER NOT NULL DEFAULT 0,
    specialist_id   TEXT,                  -- nullable, references filesystem specialist
    auto_trigger    INTEGER NOT NULL DEFAULT 0,  -- boolean: 0=off, 1=on
    created_at      TEXT NOT NULL
);
CREATE INDEX idx_columns_board ON columns(board_id);

-- Kanban: Tasks
CREATE TABLE tasks (
    id          TEXT PRIMARY KEY,
    board_id    TEXT NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
    column_id   TEXT NOT NULL REFERENCES columns(id),
    title       TEXT NOT NULL,
    description TEXT,
    position    INTEGER NOT NULL DEFAULT 0,
    status      TEXT NOT NULL DEFAULT 'active',  -- active | done | archived
    session_id  TEXT REFERENCES sessions(id),     -- nullable, agent working on it
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX idx_tasks_board ON tasks(board_id);
CREATE INDEX idx_tasks_column ON tasks(column_id);

-- Traces (immutable append-only — no updated_at)
CREATE TABLE traces (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    event_type  TEXT NOT NULL,             -- decision | tool_call | file_change | error | milestone | review
    summary     TEXT NOT NULL,
    data_json   TEXT,                      -- JSON-encoded event-specific data
    timestamp   TEXT NOT NULL
);
CREATE INDEX idx_traces_session ON traces(session_id);

-- File Changes
CREATE TABLE file_changes (
    id          TEXT PRIMARY KEY,
    trace_id    TEXT REFERENCES traces(id),  -- nullable, linked trace event
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    action      TEXT NOT NULL,             -- read | write | create | delete
    timestamp   TEXT NOT NULL
);
CREATE INDEX idx_file_changes_session ON file_changes(session_id);
