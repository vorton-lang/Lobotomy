-- The host database (data-model.md §10): state shared by all projects of this user.
-- Host databases made before migrations existed already have these tables, so they are created
-- only if missing (#16).

-- Quota domains (data-model.md §8).
CREATE TABLE IF NOT EXISTS quota_domain (
  harness    TEXT NOT NULL,
  account    TEXT NOT NULL,
  blocked_at INTEGER,
  resets_at  INTEGER,
  message    TEXT,
  PRIMARY KEY (harness, account)
) STRICT;

-- One row per harness whose permission mode the user changed (harness-adapter.md §1.9); a
-- harness without a row uses the default. The values are the adapter's, which checks them.
CREATE TABLE IF NOT EXISTS harness_setting (
  harness    TEXT PRIMARY KEY,
  permission TEXT NOT NULL,
  changed_at INTEGER NOT NULL
) STRICT;
