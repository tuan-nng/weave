-- feat-064: Add extended fields to tasks for kanban tool support.
-- priority, labels (JSON array), scope, verification_commands, test_cases.
ALTER TABLE tasks ADD COLUMN priority TEXT;
ALTER TABLE tasks ADD COLUMN labels TEXT;
ALTER TABLE tasks ADD COLUMN scope TEXT;
ALTER TABLE tasks ADD COLUMN verification_commands TEXT;
ALTER TABLE tasks ADD COLUMN test_cases TEXT;
