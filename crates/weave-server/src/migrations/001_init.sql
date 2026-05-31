-- Migration 001: Core tables
-- workspaces, sessions, messages, providers, codebases, worktrees

-- Workspaces
CREATE TABLE workspaces (
    id          TEXT PRIMARY KEY,          -- UUID v4
    name        TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active',  -- active | archived
    created_at  TEXT NOT NULL,             -- ISO 8601
    updated_at  TEXT NOT NULL              -- ISO 8601
);

-- Providers (global, not workspace-scoped)
CREATE TABLE providers (
    id          TEXT PRIMARY KEY,
    type        TEXT NOT NULL,             -- anthropic | openai | local | cli
    name        TEXT NOT NULL,
    config_json TEXT NOT NULL,             -- {base_url, api_key, default_model, ...}
    created_at  TEXT NOT NULL
);

-- Sessions
CREATE TABLE sessions (
    id                  TEXT PRIMARY KEY,
    workspace_id        TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    provider_id         TEXT NOT NULL REFERENCES providers(id),
    specialist_id       TEXT,              -- nullable, references filesystem specialist
    parent_session_id   TEXT REFERENCES sessions(id),  -- nullable, for resume
    status              TEXT NOT NULL DEFAULT 'connecting',
    model               TEXT,              -- nullable, overrides provider default
    cwd                 TEXT,              -- nullable, working directory
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);
CREATE INDEX idx_sessions_workspace ON sessions(workspace_id);
CREATE INDEX idx_sessions_status ON sessions(status);

-- Messages (immutable — no updated_at)
CREATE TABLE messages (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,             -- user | assistant | tool_use | tool_result
    content     TEXT NOT NULL,             -- JSON-encoded content
    metadata    TEXT,                      -- JSON-encoded extra data
    created_at  TEXT NOT NULL
);
CREATE INDEX idx_messages_session ON messages(session_id);

-- Codebases
CREATE TABLE codebases (
    id          TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,             -- absolute filesystem path
    branch      TEXT,                      -- nullable, defaults to current
    label       TEXT,                      -- human-readable name
    created_at  TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_codebases_path ON codebases(workspace_id, path);

-- Worktrees (reserved for v2 — no store/API code yet)
CREATE TABLE worktrees (
    id          TEXT PRIMARY KEY,
    codebase_id TEXT NOT NULL REFERENCES codebases(id) ON DELETE CASCADE,
    branch      TEXT NOT NULL,
    path        TEXT NOT NULL,             -- absolute path to worktree
    session_id  TEXT REFERENCES sessions(id),  -- nullable, linked session
    created_at  TEXT NOT NULL
);
