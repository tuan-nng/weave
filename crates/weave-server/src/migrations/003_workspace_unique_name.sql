-- Migration 003: Unique constraint on workspace name
CREATE UNIQUE INDEX idx_workspaces_name ON workspaces(name);
