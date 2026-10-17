-- Tasks a worker runs (tech-plan.md §7 step 1, group 7): what the orchestrator knows of each from
-- the worker's events. The task row keeps the title, the user's words and the phase; these keep
-- the rest.

CREATE TABLE worker_task (
  task_id      TEXT PRIMARY KEY REFERENCES task (id),
  worker       TEXT NOT NULL,
  spec_version INTEGER NOT NULL,
  -- The current attempt, as the worker named it.
  attempt_id   TEXT NOT NULL,
  -- Its state, why it is blocked and its open question, as the worker last said, as JSON.
  status       TEXT NOT NULL,
  -- The worker's last event applied.
  seq          INTEGER NOT NULL DEFAULT 0
) STRICT;

-- The current version of each artifact, as the protocol's artifact in JSON.
CREATE TABLE worker_artifact (
  task_id     TEXT NOT NULL REFERENCES task (id),
  artifact_id TEXT NOT NULL,
  attempt_id  TEXT NOT NULL,
  current     TEXT NOT NULL,
  PRIMARY KEY (task_id, artifact_id)
) STRICT;

-- The evidence on each artifact version, as the protocol's evidence in JSON.
CREATE TABLE worker_evidence (
  task_id     TEXT NOT NULL REFERENCES task (id),
  artifact_id TEXT NOT NULL,
  version     TEXT NOT NULL,
  name        TEXT NOT NULL,
  current     TEXT NOT NULL,
  PRIMARY KEY (task_id, artifact_id, version, name)
) STRICT;

-- A role a worker plays: the backend adds one for each worker of the project (workers.toml). Roles
-- that run on the backend itself no longer take tasks.
ALTER TABLE role ADD COLUMN remote INTEGER NOT NULL DEFAULT 0 CHECK (remote IN (0, 1));
