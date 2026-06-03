-- Migration 009: Add context_id to sessions for A2A task linking.
--
-- contextId (from the A2A protocol) groups multiple sessions into
-- one logical A2A task. A session with context_id = NULL is a
-- standalone session (normal chat). When an A2A SendMessage carries
-- a contextId, all sessions spawned for the same task share the
-- same context_id, enabling multi-session task lifecycles.
--
-- The index on context_id supports efficient "find all sessions for
-- this A2A task" queries without a full scan.

ALTER TABLE sessions ADD COLUMN context_id TEXT;

CREATE INDEX IF NOT EXISTS idx_sessions_context_id ON sessions(context_id);
