-- What a closed task leaves behind and what a turn outside any task changes (#14).

-- A task's messages still queued when it closes are not delivered: 'dropped'
-- (data-model.md §4.2). SQLite cannot change a CHECK constraint, so the table is rebuilt.
CREATE TABLE message_new (
  id         TEXT PRIMARY KEY,
  seq        INTEGER NOT NULL UNIQUE,
  role       TEXT NOT NULL REFERENCES role (name),
  source     TEXT NOT NULL,
  task_id    TEXT REFERENCES task (id),
  body       TEXT NOT NULL,
  state      TEXT NOT NULL CHECK (state IN ('queued', 'bound', 'dropped')),
  turn_id    TEXT,
  created_at INTEGER NOT NULL,
  -- From 0002_turns.sql.
  delivered_at INTEGER
) STRICT;
INSERT INTO message_new (id, seq, role, source, task_id, body, state, turn_id, created_at, delivered_at)
  SELECT id, seq, role, source, task_id, body, state, turn_id, created_at, delivered_at FROM message;
DROP TABLE message;
ALTER TABLE message_new RENAME TO message;
CREATE INDEX message_queued ON message (role, seq) WHERE state = 'queued';

-- Changes a turn outside any task left in the slot wait for the user: made into a task, or
-- discarded (harness-adapter.md §3).
ALTER TABLE capture ADD COLUMN outside TEXT CHECK (outside IN ('pending', 'adopted', 'discarded'));
-- A task made from such changes starts from them.
ALTER TABLE task ADD COLUMN origin_capture TEXT REFERENCES capture (id);
