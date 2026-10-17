-- The environment's part (worker-interface.md §6): an attempt's port range, once its clone is
-- ready; the review round whose delivery the environment has taken; the artifacts published.

-- The first of the attempt's ports. Set once the attempt is prepared.
ALTER TABLE attempt ADD COLUMN port_base INTEGER;
-- The instance's attempt (a review round) whose delivery has been taken.
ALTER TABLE attempt ADD COLUMN delivered_round TEXT;

CREATE TABLE artifact (
  id         TEXT PRIMARY KEY,
  task_id    TEXT NOT NULL REFERENCES task (id),
  attempt_id TEXT NOT NULL REFERENCES attempt (id),
  kind       TEXT NOT NULL,
  -- The current version, as the protocol's artifact in JSON.
  current    TEXT NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;
