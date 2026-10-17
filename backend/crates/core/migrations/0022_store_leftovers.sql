-- What 0021 had to leave: the private store's columns, then the tables only they pointed to (#86).
-- 'verifying' stays in task.phase's CHECK until the task phases change with the worker layer.
ALTER TABLE attempt DROP COLUMN code_start;
ALTER TABLE attempt DROP COLUMN candidate_id;
ALTER TABLE attempt DROP COLUMN conflicts;
ALTER TABLE attempt DROP COLUMN trial;
ALTER TABLE task DROP COLUMN origin_capture;
ALTER TABLE turn DROP COLUMN workspace_id;
ALTER TABLE project DROP COLUMN branch;
ALTER TABLE project DROP COLUMN author_name;
ALTER TABLE project DROP COLUMN author_email;
ALTER TABLE project DROP COLUMN integration;
ALTER TABLE project DROP COLUMN integration_rev;
ALTER TABLE project DROP COLUMN previewed;
ALTER TABLE role DROP COLUMN slot;

DROP TABLE capture;
DROP TABLE workspace;
DROP TABLE project_config;
