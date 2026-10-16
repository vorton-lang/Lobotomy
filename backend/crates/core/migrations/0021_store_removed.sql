-- The private store went with its captures, slots, verification, previews and display-branch
-- adoption (#86). A task that waited in verification now waits for acceptance.
UPDATE task SET phase = 'accepting' WHERE phase = 'verifying';

-- Tables nothing points to go. capture, workspace and project_config stay, unused, because other
-- tables still point to them; so do the store's columns, until the schema is replaced (#86).
DROP TABLE IF EXISTS check_run;
DROP TABLE IF EXISTS verification;
DROP TABLE IF EXISTS publication;
DROP TABLE IF EXISTS outbox;
DROP TABLE IF EXISTS display_adoption;
DROP TABLE IF EXISTS display_observation;
