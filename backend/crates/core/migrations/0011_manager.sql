-- The manager's commands in a project (data-model.md §10.6; roles-and-tasks.md §4; M4 step 2).

-- Who paused the task, as a caller scope (`user` or `manager`): only the user lifts the user's
-- pause. Before this migration only the user could pause.
ALTER TABLE task ADD COLUMN paused_by TEXT;
UPDATE task SET paused_by = 'user' WHERE paused = 1;

-- What the manager's assign adds to a task, as JSON: the actions it allows, when to stop, and her
-- interpretation. NULL for a task the user created.
ALTER TABLE task ADD COLUMN assignment TEXT;

-- The user's words the manager quoted, copied from her database with their id there
-- (data-model.md §10.6 跨库操作第 1 条). Each belongs to an assigned task or to a message.
CREATE TABLE quote (
  id         TEXT PRIMARY KEY,
  source_id  TEXT NOT NULL,
  said_at    INTEGER NOT NULL,
  body       TEXT NOT NULL,
  task_id    TEXT REFERENCES task (id),
  message_id TEXT REFERENCES message (id),
  pos        INTEGER NOT NULL,
  CHECK ((task_id IS NULL) <> (message_id IS NULL))
) STRICT;
CREATE INDEX quote_task ON quote (task_id) WHERE task_id IS NOT NULL;
CREATE INDEX quote_message ON quote (message_id) WHERE message_id IS NOT NULL;
