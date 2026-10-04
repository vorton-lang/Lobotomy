-- The work a slot was written for: the task, or NULL for a role without a task. A slot whose
-- task is no longer the role's work is written again at the integration version
-- (harness-adapter.md §3).
ALTER TABLE workspace ADD COLUMN task_id TEXT REFERENCES task (id);
