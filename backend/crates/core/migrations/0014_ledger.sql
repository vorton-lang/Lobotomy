-- The manager's ledger (manager-actions.md §3): append-only entries, each in the database of its
-- scope (data-model.md §10.6 "记录按作用域存放"). The user's scope is in her database; a project's
-- and a task's are in the project's. A change is a new entry that supersedes the old one.
CREATE TABLE ledger_entry (
  id         TEXT PRIMARY KEY,
  kind       TEXT NOT NULL
             CHECK (kind IN ('goal', 'phase', 'decision', 'constraint', 'open_question', 'hypothesis')),
  statement  TEXT NOT NULL,
  -- Set for an entry of a task's scope; it lapses when the task closes (roles-and-tasks.md §2.2).
  task_id    TEXT REFERENCES task (id),
  -- For a hypothesis: how sure she is, in her words.
  confidence TEXT,
  supersedes TEXT UNIQUE REFERENCES ledger_entry (id),
  turn_id    TEXT NOT NULL,
  created_at INTEGER NOT NULL
) STRICT;

-- The user's words an entry rests on, copied from her database with their id there
-- (data-model.md §10.6 跨库操作第 1 条). A decision and a constraint need at least one.
CREATE TABLE ledger_evidence (
  entry_id  TEXT NOT NULL REFERENCES ledger_entry (id),
  pos       INTEGER NOT NULL,
  source_id TEXT NOT NULL,
  said_at   INTEGER NOT NULL,
  body      TEXT NOT NULL,
  PRIMARY KEY (entry_id, pos)
) STRICT;
