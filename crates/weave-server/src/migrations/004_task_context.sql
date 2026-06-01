-- Migration 004: Add task context columns
-- acceptance_criteria: what "done" looks like (set at task creation or by planner)
-- completion_summary: agent's self-report of what was done
-- verification_report: evidence that acceptance criteria are met

ALTER TABLE tasks ADD COLUMN acceptance_criteria TEXT;
ALTER TABLE tasks ADD COLUMN completion_summary TEXT;
ALTER TABLE tasks ADD COLUMN verification_report TEXT;
