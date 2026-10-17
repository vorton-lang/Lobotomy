-- The forge's part (worker-interface.md §6): an attempt's pull request, the CI rounds handed back
-- to its agent, its adoption; and the evidence on each artifact version.

ALTER TABLE attempt ADD COLUMN pr_number INTEGER;
ALTER TABLE attempt ADD COLUMN pr_url TEXT;
-- How many times a CI failure went back to the agent, and on which version the last time.
ALTER TABLE attempt ADD COLUMN ci_rounds INTEGER NOT NULL DEFAULT 0;
ALTER TABLE attempt ADD COLUMN handed_back TEXT;
-- The artifact version adopted, as "<artifact_id>@<version>".
ALTER TABLE attempt ADD COLUMN adopted TEXT;

CREATE TABLE evidence (
  artifact_id TEXT NOT NULL,
  version     TEXT NOT NULL,
  name        TEXT NOT NULL,
  task_id     TEXT NOT NULL REFERENCES task (id),
  -- As the protocol's evidence in JSON.
  current     TEXT NOT NULL,
  updated_at  INTEGER NOT NULL,
  PRIMARY KEY (artifact_id, version, name)
) STRICT;
