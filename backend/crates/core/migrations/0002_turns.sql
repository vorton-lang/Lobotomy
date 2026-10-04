-- M1 schema, part 2: native sessions, turns, message delivery, threads and items
-- (notes/data-model.md §2, §3, §7).

CREATE TABLE native_session (
  id         TEXT PRIMARY KEY,
  role       TEXT NOT NULL REFERENCES role (name),
  harness    TEXT NOT NULL CHECK (harness IN ('claude', 'codex')),
  -- The harness's own session id. Codex reports it in the first turn (harness-adapter.md §1.3).
  native_id  TEXT,
  started_at INTEGER NOT NULL,
  ended_at   INTEGER
) STRICT;
-- Each role has at most one current native session.
CREATE UNIQUE INDEX native_session_current ON native_session (role) WHERE ended_at IS NULL;

CREATE TABLE turn (
  id                TEXT PRIMARY KEY,
  role              TEXT NOT NULL REFERENCES role (name),
  native_session_id TEXT NOT NULL REFERENCES native_session (id),
  task_id           TEXT REFERENCES task (id),
  attempt_id        TEXT REFERENCES attempt (id),
  token             TEXT NOT NULL UNIQUE,
  input             TEXT NOT NULL,
  state             TEXT NOT NULL CHECK (state IN ('registered', 'running', 'ended', 'unknown')),
  outcome           TEXT CHECK (outcome IN ('completed', 'failed', 'interrupted')),
  failure           TEXT,
  pid               INTEGER,
  -- OS process start time; tells the CLI apart from a later process that reuses its pid.
  process_start     INTEGER,
  done_at           INTEGER,
  registered_at     INTEGER NOT NULL,
  started_at        INTEGER,
  ended_at          INTEGER,
  CHECK ((state = 'ended') = (outcome IS NOT NULL))
) STRICT;
-- Each native session has at most one unfinished turn (data-model.md §2).
CREATE UNIQUE INDEX turn_one_unfinished_per_session ON turn (native_session_id) WHERE state <> 'ended';
CREATE INDEX turn_by_session ON turn (native_session_id, registered_at);

-- Set when the CLI has started and its stdin was written and closed (data-model.md §3.4).
ALTER TABLE message ADD COLUMN delivered_at INTEGER;

-- The turn that reported `done` in this attempt. Its capture becomes the candidate.
ALTER TABLE attempt ADD COLUMN done_turn_id TEXT REFERENCES turn (id);

-- Role commands record the turn they came from.
ALTER TABLE command_record ADD COLUMN turn_id TEXT;

CREATE TABLE thread (
  id   TEXT PRIMARY KEY,
  role TEXT NOT NULL UNIQUE REFERENCES role (name)
) STRICT;

CREATE TABLE item (
  id             TEXT PRIMARY KEY,
  thread_id      TEXT NOT NULL REFERENCES thread (id),
  seq            INTEGER NOT NULL,
  turn_id        TEXT NOT NULL REFERENCES turn (id),
  native_item_id TEXT,
  kind           TEXT NOT NULL,
  content        TEXT NOT NULL,
  command_id     TEXT REFERENCES command_record (id),
  created_at     INTEGER NOT NULL,
  UNIQUE (thread_id, seq),
  UNIQUE (turn_id, native_item_id)
) STRICT;

-- Content-addressed files for fields over the storage threshold (data-model.md §7.3).
CREATE TABLE blob (
  hash       TEXT PRIMARY KEY,
  size       INTEGER NOT NULL,
  head       TEXT NOT NULL,
  tail       TEXT NOT NULL,
  created_at INTEGER NOT NULL
) STRICT;
