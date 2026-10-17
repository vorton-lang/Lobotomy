-- What a message to a worker's task replies to, fixed when it is written (worker-interface.md §3):
-- the worker's question it answers, and the stop it continues. Sending it again sends the same.
ALTER TABLE message ADD COLUMN question_id TEXT;
ALTER TABLE message ADD COLUMN stop_id TEXT;
