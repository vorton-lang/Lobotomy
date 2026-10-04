-- The files an attempt's code start left with conflict markers, as a JSON array; NULL for none.
-- A new native session in the middle of the attempt is told about them again, with the brief
-- (data-model.md §4.1, #16).
ALTER TABLE attempt ADD COLUMN conflicts TEXT;
