-- What the executor's done said about the work: the report's title and body, an empty one left
-- out. The candidate's commit message uses it (harness-adapter.md §4.3). It was read back from
-- the command log, where renaming a report argument would drop it without a word (#16).
ALTER TABLE attempt ADD COLUMN done_summary TEXT;

-- An attempt whose done came before this migration: the same text, from the command log.
UPDATE attempt SET done_summary = (
  SELECT nullif(trim(
           trim(coalesce(json_extract(args, '$.title'), ''), char(32, 9, 10, 13)) || char(10, 10) ||
           trim(coalesce(json_extract(args, '$.body'), ''), char(32, 9, 10, 13)),
           char(10)), '')
  FROM command_record
  WHERE turn_id = attempt.done_turn_id AND name = 'org_report' AND json_extract(args, '$.status') = 'done'
  ORDER BY created_at LIMIT 1
)
WHERE done_turn_id IS NOT NULL;
