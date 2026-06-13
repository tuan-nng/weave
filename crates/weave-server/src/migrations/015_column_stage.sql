-- Migration 015: Per-column stage enum for kanban coordination.
--
-- Adds a `stage` column to `columns` that controls prompt variation,
-- available tools, and allowed target-column transitions. The five
-- stages are: backlog, todo, dev, review, done.
--
-- Heuristic backfill maps known column names to stages:
--   Backlog → backlog, To Do → todo, In Progress → dev,
--   Review → review, Done → done.
-- Non-standard names default to 'dev' (most permissive).

ALTER TABLE columns ADD COLUMN stage TEXT NOT NULL DEFAULT 'dev';

UPDATE columns SET stage = 'backlog' WHERE name = 'Backlog';
UPDATE columns SET stage = 'todo' WHERE name = 'To Do';
UPDATE columns SET stage = 'dev' WHERE name = 'In Progress';
UPDATE columns SET stage = 'review' WHERE name = 'Review';
UPDATE columns SET stage = 'done' WHERE name = 'Done';
