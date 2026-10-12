-- Worker and manager questions share one lifecycle. Message seq is allocated under the same
-- write transaction as a question's boundary; wall clocks and delivery order are irrelevant.
ALTER TABLE question ADD COLUMN role TEXT REFERENCES role (name);
ALTER TABLE question ADD COLUMN task_id TEXT REFERENCES task (id);
ALTER TABLE question ADD COLUMN after_message_seq INTEGER NOT NULL DEFAULT 0;
UPDATE question SET role = (SELECT role FROM turn WHERE turn.id = question.turn_id);

-- Recover the enqueue boundary from the ordered event log, not from timestamps. If an old
-- record has no asking event, retain the question and treat existing messages as context.
UPDATE question SET after_message_seq = CASE
  WHEN EXISTS (SELECT 1 FROM event WHERE kind = 'question.asked' AND entity = question.id)
  THEN (SELECT COALESCE(MAX(m.seq), 0) FROM message m JOIN event e ON e.entity = m.id
        WHERE e.kind = 'message.queued' AND e.seq <
          (SELECT MIN(seq) FROM event WHERE kind = 'question.asked' AND entity = question.id))
  ELSE (SELECT COALESCE(MAX(seq), 0) FROM message) END;

INSERT INTO question (id, body, turn_id, created_at, role, task_id, after_message_seq)
SELECT 'q_legacy_' || t.id, t.blocked_reason,
       COALESCE((SELECT turn_id FROM command_record WHERE id = e.command_id),
                (SELECT id FROM turn WHERE task_id = t.id ORDER BY registered_at DESC, id DESC LIMIT 1), ''),
       COALESCE(e.created_at, t.created_at), t.executor, t.id,
       CASE WHEN e.seq IS NULL THEN (SELECT COALESCE(MAX(seq), 0) FROM message)
            ELSE (SELECT COALESCE(MAX(m.seq), 0) FROM message m JOIN event queued ON queued.entity = m.id
                  WHERE queued.kind = 'message.queued' AND queued.seq < e.seq) END
FROM task t LEFT JOIN event e ON e.seq =
  (SELECT MAX(seq) FROM event WHERE kind = 'task.blocked_changed' AND entity = t.id
   AND json_extract(payload, '$.blocked_reason') = t.blocked_reason)
WHERE t.blocked_reason IS NOT NULL;

CREATE INDEX question_open_by_role ON question (role) WHERE closed_at IS NULL;
CREATE UNIQUE INDEX question_open_by_task ON question (task_id) WHERE closed_at IS NULL;
