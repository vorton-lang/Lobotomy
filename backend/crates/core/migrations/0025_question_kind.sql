-- A question says what kind of answer it needs and may offer options (tech-plan.md §5 "要你卡").
-- A message's question_id also names the manager's question a user's reply on its card answers.
ALTER TABLE question ADD COLUMN kind TEXT NOT NULL DEFAULT 'open' CHECK (kind IN ('confirm', 'choice', 'open'));
ALTER TABLE question ADD COLUMN options TEXT NOT NULL DEFAULT '[]';
