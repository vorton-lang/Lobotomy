-- How far the manager has read each project, in her own database (data-model.md §10.6 跨库操作
-- 第 4 条): the last event of the project the runtime put in her input (manager-actions.md §6).
-- Project databases share the schema and leave the table empty.
CREATE TABLE project_cursor (
  project_id TEXT PRIMARY KEY,
  seq        INTEGER NOT NULL
) STRICT;
