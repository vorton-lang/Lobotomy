-- M1 schema, part 3: the project and its integration version, project configuration, execution
-- sites, captures, verification, check runs and the outbox (data-model.md §5, §6, §9;
-- harness-adapter.md §3, §4).

-- One project per instance (data-model.md §10). It holds the integration version: the head only
-- moves forward, by accepting a candidate, with a compare-and-set on the head (harness-adapter.md
-- §4.2).
CREATE TABLE project (
  id              TEXT PRIMARY KEY,
  repo_path       TEXT NOT NULL,
  branch          TEXT NOT NULL,
  -- The user's git identity; integration versions are authored by the user (harness-adapter.md §4.3).
  author_name     TEXT NOT NULL,
  author_email    TEXT NOT NULL,
  integration     TEXT NOT NULL,
  integration_rev INTEGER NOT NULL,
  -- The integration version the user's repository was last brought to.
  previewed       TEXT NOT NULL,
  created_at      INTEGER NOT NULL
) STRICT;

-- Every integration version after the first, in order.
CREATE TABLE publication (
  rev         INTEGER PRIMARY KEY,
  commit_id   TEXT NOT NULL,
  previous    TEXT NOT NULL,
  task_id     TEXT NOT NULL REFERENCES task (id),
  decision_id TEXT NOT NULL REFERENCES decision (id),
  created_at  INTEGER NOT NULL
) STRICT;

-- Append-only; check results name the version they ran with (data-model.md §9.1).
CREATE TABLE project_config (
  version    INTEGER PRIMARY KEY,
  config     TEXT NOT NULL,
  created_by TEXT NOT NULL,
  created_at INTEGER NOT NULL
) STRICT;

-- A slot directory and its generation (harness-adapter.md §3; data-model.md §5, §6). A row is
-- one generation; rematerializing in place updates its target. A directory that could not be
-- brought to its target is moved aside and replaced by the next generation.
CREATE TABLE workspace (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  generation INTEGER NOT NULL,
  -- The commit written into the directory, and git's HEAD there: the baseline.
  target     TEXT NOT NULL,
  head       TEXT NOT NULL,
  state      TEXT NOT NULL CHECK (state IN ('materializing', 'ready', 'retired')),
  created_at INTEGER NOT NULL,
  ready_at   INTEGER,
  UNIQUE (name, generation)
) STRICT;
CREATE UNIQUE INDEX workspace_current ON workspace (name) WHERE state <> 'retired';

ALTER TABLE turn ADD COLUMN workspace_id TEXT REFERENCES workspace (id);

-- One capture per turn, made after the CLI exited (harness-adapter.md §4.1). The pin in the store
-- is named after the capture's id.
CREATE TABLE capture (
  id             TEXT PRIMARY KEY,
  turn_id        TEXT NOT NULL UNIQUE REFERENCES turn (id),
  role           TEXT NOT NULL REFERENCES role (name),
  task_id        TEXT REFERENCES task (id),
  attempt_id     TEXT REFERENCES attempt (id),
  kind           TEXT NOT NULL CHECK (kind IN ('turn', 'candidate', 'interrupted')),
  workspace_id   TEXT NOT NULL REFERENCES workspace (id),
  base           TEXT NOT NULL,
  config_version INTEGER NOT NULL REFERENCES project_config (version),
  state          TEXT NOT NULL CHECK (state IN ('intent', 'pinned', 'oversized', 'uncovered')),
  -- The user's decision for a retry: keep new files over the guardrail, or leave out what cannot
  -- be captured.
  options        TEXT NOT NULL DEFAULT '{}',
  commit_id      TEXT,
  detail         TEXT,
  created_at     INTEGER NOT NULL,
  finished_at    INTEGER,
  CHECK ((state = 'pinned') = (commit_id IS NOT NULL))
) STRICT;
CREATE INDEX capture_by_role ON capture (role, created_at);

-- Where an attempt's code starts (NULL: the slot stays as it is, data-model.md §4.3), and the
-- candidate it produced.
ALTER TABLE attempt ADD COLUMN code_start TEXT;
ALTER TABLE attempt ADD COLUMN candidate_id TEXT REFERENCES capture (id);

CREATE TABLE verification (
  id             TEXT PRIMARY KEY,
  task_id        TEXT NOT NULL REFERENCES task (id),
  attempt_id     TEXT NOT NULL REFERENCES attempt (id),
  capture_id     TEXT NOT NULL REFERENCES capture (id),
  -- The integration version the candidate was rebased onto, and the commit that resulted. That
  -- commit becomes the next integration version if the user accepts it.
  base           TEXT NOT NULL,
  config_version INTEGER NOT NULL REFERENCES project_config (version),
  commit_id      TEXT,
  conflicts      TEXT,
  state          TEXT NOT NULL CHECK (state IN ('running', 'passed', 'failed')),
  created_at     INTEGER NOT NULL,
  finished_at    INTEGER
) STRICT;
CREATE UNIQUE INDEX verification_one_running ON verification (task_id) WHERE state = 'running';

CREATE TABLE check_run (
  id              TEXT PRIMARY KEY,
  verification_id TEXT NOT NULL REFERENCES verification (id),
  seq             INTEGER NOT NULL,
  command         TEXT NOT NULL,
  exit_code       INTEGER,
  timed_out       INTEGER NOT NULL CHECK (timed_out IN (0, 1)),
  output          TEXT NOT NULL,
  duration_ms     INTEGER NOT NULL,
  UNIQUE (verification_id, seq)
) STRICT;

-- Side effects written in the transaction that causes them (data-model.md §5).
CREATE TABLE outbox (
  id         TEXT PRIMARY KEY,
  kind       TEXT NOT NULL,
  idem_key   TEXT NOT NULL UNIQUE,
  payload    TEXT NOT NULL,
  state      TEXT NOT NULL CHECK (state IN ('pending', 'done', 'stopped', 'superseded')),
  receipt    TEXT,
  created_at INTEGER NOT NULL,
  done_at    INTEGER
) STRICT;
CREATE INDEX outbox_open ON outbox (kind, created_at) WHERE state IN ('pending', 'stopped');
