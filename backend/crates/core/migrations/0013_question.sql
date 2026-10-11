-- The manager's questions to the user (manager-actions.md §6 `ask_user`), in her own database.
-- One stays in "等你决定" until a message of the user's sent after it is delivered to her
-- (`closed_in`, the turn that delivered it). Project databases share the schema and leave the
-- table empty.
CREATE TABLE question (
  id         TEXT PRIMARY KEY,
  body       TEXT NOT NULL,
  turn_id    TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  closed_in  TEXT,
  closed_at  INTEGER
) STRICT;
CREATE INDEX question_open ON question (created_at) WHERE closed_at IS NULL;
