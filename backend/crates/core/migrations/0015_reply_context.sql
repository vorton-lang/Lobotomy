-- What the user's message replies to, when the user chose it (manager-actions.md §1.1): an item or
-- a message of the same database, such as one of the manager's replies. Every role follows it.
ALTER TABLE message ADD COLUMN reply_to TEXT;

-- What the user was replying to, as the manager forwards it with the user's words: copied in with
-- its id where it came from (data-model.md §10.6 跨库操作第 1 条). Each belongs to an assigned task
-- or to a message, as a quote does.
CREATE TABLE forward_context (
  id         TEXT PRIMARY KEY,
  source_id  TEXT NOT NULL,
  header     TEXT NOT NULL,
  body       TEXT NOT NULL,
  -- The user chose it; the manager could not leave it out.
  chosen     INTEGER NOT NULL CHECK (chosen IN (0, 1)),
  task_id    TEXT REFERENCES task (id),
  message_id TEXT REFERENCES message (id),
  pos        INTEGER NOT NULL,
  CHECK ((task_id IS NULL) <> (message_id IS NULL))
) STRICT;
