-- #67: observation never moves the integration head. Adoption is a separate source of revisions,
-- not a worker publication. Old publication rows and all task evidence remain historical.
CREATE TABLE display_observation (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  previewed TEXT NOT NULL,
  state TEXT NOT NULL
) STRICT;
CREATE TABLE display_adoption (
  id TEXT PRIMARY KEY,
  target TEXT NOT NULL,
  previous TEXT NOT NULL,
  expected_rev INTEGER NOT NULL,
  config_version INTEGER NOT NULL REFERENCES project_config (version),
  state TEXT NOT NULL CHECK (state IN ('pending', 'applied', 'failed')),
  checks TEXT NOT NULL DEFAULT '[]',
  error TEXT,
  created_at INTEGER NOT NULL,
  finished_at INTEGER
) STRICT;
CREATE UNIQUE INDEX display_adoption_pending ON display_adoption (state) WHERE state = 'pending';
