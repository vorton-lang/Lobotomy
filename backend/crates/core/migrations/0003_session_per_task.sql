-- Native sessions belong to a task (#11): the executor starts a fresh session for each task and
-- the session ends when the task closes. Messages outside any task use a session without a task.

ALTER TABLE native_session ADD COLUMN task_id TEXT REFERENCES task (id);

DROP INDEX native_session_current;
-- Each role has at most one current session per task, and one outside any task.
CREATE UNIQUE INDEX native_session_current ON native_session (role, COALESCE(task_id, '')) WHERE ended_at IS NULL;

-- One role, one slot: at most one unfinished turn per role, whatever its session. With a session
-- per task, a per-session barrier would let a new task start a second CLI in the same slot while
-- the old one still runs (data-model.md §2).
DROP INDEX turn_one_unfinished_per_session;
CREATE UNIQUE INDEX turn_one_unfinished_per_role ON turn (role) WHERE state <> 'ended';
