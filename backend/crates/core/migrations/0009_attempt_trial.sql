-- An optional manual trial suggested by the first accepted done. This is metadata only;
-- existing attempts have no suggestion, and recording one never launches a command.
ALTER TABLE attempt ADD COLUMN trial TEXT;
