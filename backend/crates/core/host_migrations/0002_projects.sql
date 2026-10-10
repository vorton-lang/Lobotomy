-- The project registry (data-model.md §10.3). The repository path is copied here when a project
-- is registered: the uniqueness check must also see archived projects and projects that cannot be
-- opened. The project's own database stays authoritative for it.
CREATE TABLE project (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  data_dir      TEXT NOT NULL UNIQUE,
  repo_path     TEXT NOT NULL,
  state         TEXT NOT NULL CHECK (state IN ('onboarding', 'running', 'archived')),
  registered_at INTEGER NOT NULL
) STRICT;

-- One unarchived project per repository: two would write their previews into the same branch.
CREATE UNIQUE INDEX project_unarchived_repo ON project (repo_path) WHERE state <> 'archived';
