-- A role's model and reasoning effort, one choice per harness (harness-adapter.md §1.2): model
-- names belong to a harness, so a role that switches harness and back finds its choice again.
-- NULL leaves the choice to the CLI's default.
CREATE TABLE role_model (
  role    TEXT NOT NULL REFERENCES role (name),
  harness TEXT NOT NULL CHECK (harness IN ('claude', 'codex')),
  model   TEXT,
  effort  TEXT,
  PRIMARY KEY (role, harness)
) STRICT;

INSERT INTO role_model (role, harness, model) SELECT name, harness, model FROM role WHERE model IS NOT NULL;
ALTER TABLE role DROP COLUMN model;
