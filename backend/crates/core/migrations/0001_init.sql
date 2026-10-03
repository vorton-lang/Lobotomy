-- M1 schema, part 1: roles, tasks, attempts, occupancy, messages, decisions, the command log and
-- the global event log (notes/data-model.md §9.1). Later increments add turns, captures and more.

CREATE TABLE role (
  name    TEXT PRIMARY KEY,
  kind    TEXT NOT NULL CHECK (kind IN ('manager', 'tech_lead', 'worker', 'reviewer')),
  harness TEXT NOT NULL CHECK (harness IN ('claude', 'codex')),
  model   TEXT,
  slot    TEXT
) STRICT;

CREATE TABLE task (
  id               TEXT PRIMARY KEY,
  title            TEXT NOT NULL,
  body             TEXT NOT NULL,
  executor         TEXT NOT NULL REFERENCES role (name),
  phase            TEXT NOT NULL
                   CHECK (phase IN ('queued', 'executing', 'verifying', 'accepting', 'done', 'abandoned')),
  paused           INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0, 1)),
  blocked_reason   TEXT,
  criteria_version INTEGER NOT NULL,
  queue_pos        INTEGER,
  revision         INTEGER NOT NULL,
  created_at       INTEGER NOT NULL,
  closed_at        INTEGER
) STRICT;

CREATE TABLE criteria_version (
  task_id    TEXT NOT NULL REFERENCES task (id),
  version    INTEGER NOT NULL,
  text       TEXT NOT NULL,
  created_by TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (task_id, version)
) STRICT;

CREATE TABLE attempt (
  id         TEXT PRIMARY KEY,
  task_id    TEXT NOT NULL REFERENCES task (id),
  seq        INTEGER NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at   INTEGER,
  end_reason TEXT CHECK (end_reason IN ('candidate', 'abandoned')),
  UNIQUE (task_id, seq)
) STRICT;
-- Each task has at most one open attempt.
CREATE UNIQUE INDEX attempt_one_open_per_task ON attempt (task_id) WHERE ended_at IS NULL;

-- Task-level occupancy: each role is occupied by at most one task (data-model.md §2).
CREATE TABLE occupancy (
  role    TEXT PRIMARY KEY REFERENCES role (name),
  task_id TEXT NOT NULL REFERENCES task (id),
  since   INTEGER NOT NULL
) STRICT;

CREATE TABLE message (
  id         TEXT PRIMARY KEY,
  seq        INTEGER NOT NULL UNIQUE,
  role       TEXT NOT NULL REFERENCES role (name),
  source     TEXT NOT NULL,
  task_id    TEXT REFERENCES task (id),
  body       TEXT NOT NULL,
  state      TEXT NOT NULL CHECK (state IN ('queued', 'bound')),
  turn_id    TEXT,
  created_at INTEGER NOT NULL
) STRICT;
CREATE INDEX message_queued ON message (role, seq) WHERE state = 'queued';

CREATE TABLE decision (
  id         TEXT PRIMARY KEY,
  kind       TEXT NOT NULL,
  task_id    TEXT REFERENCES task (id),
  actor      TEXT NOT NULL,
  detail     TEXT NOT NULL,
  created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE command_record (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  caller     TEXT NOT NULL,
  idem_key   TEXT,
  args       TEXT NOT NULL,
  result     TEXT NOT NULL,
  created_at INTEGER NOT NULL
) STRICT;
CREATE UNIQUE INDEX command_idem ON command_record (caller, idem_key) WHERE idem_key IS NOT NULL;

CREATE TABLE event (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  kind       TEXT NOT NULL,
  entity     TEXT NOT NULL,
  payload    TEXT NOT NULL,
  created_at INTEGER NOT NULL
) STRICT;

-- v1 staff that M1 needs (roles-and-tasks.md §1.2). Other roles arrive with their milestones.
INSERT INTO role (name, kind, harness, model, slot) VALUES ('Malkuth', 'worker', 'codex', NULL, 'worker');
