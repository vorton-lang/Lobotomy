-- The command that wrote each event, so that a reader can tell whether the user, the runtime or a
-- role caused it (data-model.md §10.6). Events from before this migration name no command.
-- A command writes its events before its command record, in the same transaction (db.rs), so the
-- reference is checked when the transaction commits.
ALTER TABLE event ADD COLUMN command_id TEXT REFERENCES command_record (id) DEFERRABLE INITIALLY DEFERRED;
