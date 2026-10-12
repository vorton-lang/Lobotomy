-- Project overviews read each task's latest verification and undelivered messages. The
-- running-verification and role-inbox indexes do not serve these task-scoped reads.
CREATE INDEX verification_by_task ON verification (task_id, created_at, id);
CREATE INDEX message_queued_by_task ON message (task_id, seq) WHERE state = 'queued';
