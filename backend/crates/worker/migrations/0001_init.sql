-- The worker host's own records (worker-interface.md §2, §5, §9). A task's turns, messages and
-- questions are in its instance's database; these say what the protocol made of them.

CREATE TABLE task (
  id           TEXT PRIMARY KEY,
  -- create_task's params as JSON: a retry repeats them, a reuse of the id does not.
  request      TEXT NOT NULL,
  spec_version INTEGER NOT NULL,
  spec         TEXT NOT NULL,
  -- The current attempt.
  attempt_id   TEXT NOT NULL,
  -- What the orchestrator was last told of the current attempt, as JSON: its state, the open
  -- question.
  status       TEXT NOT NULL,
  question     TEXT,
  closed       TEXT CHECK (closed IN ('adopted', 'abandoned')),
  -- The task's last event.
  seq          INTEGER NOT NULL,
  -- The instance's last event read for mismatches.
  core_seq     INTEGER NOT NULL DEFAULT 0,
  created_at   INTEGER NOT NULL
) STRICT;

CREATE TABLE attempt (
  id           TEXT PRIMARY KEY,
  task_id      TEXT NOT NULL REFERENCES task (id),
  seq          INTEGER NOT NULL,
  spec_version INTEGER NOT NULL,
  -- The instance's task the attempt runs as.
  core_task    TEXT NOT NULL,
  -- The end asked for. It is reached once no turn of the attempt is left unfinished.
  ending       TEXT CHECK (ending IN ('canceled', 'abandoned')),
  -- The final state, once reached, as JSON.
  ended        TEXT,
  -- What revise returned when it started this attempt: the superseded messages, as JSON.
  superseded   TEXT NOT NULL DEFAULT '[]',
  started_at   INTEGER NOT NULL,
  UNIQUE (task_id, seq),
  UNIQUE (task_id, spec_version)
) STRICT;

CREATE TABLE message (
  id          TEXT PRIMARY KEY,
  task_id     TEXT NOT NULL REFERENCES task (id),
  attempt_id  TEXT NOT NULL REFERENCES attempt (id),
  request     TEXT NOT NULL,
  -- The instance's message it became.
  core_id     TEXT NOT NULL,
  -- The receipt last reported, as JSON.
  receipt     TEXT NOT NULL,
  created_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE event (
  task_id    TEXT NOT NULL REFERENCES task (id),
  seq        INTEGER NOT NULL,
  attempt_id TEXT NOT NULL,
  body       TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (task_id, seq)
) STRICT;
