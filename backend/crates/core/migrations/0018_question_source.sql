-- 0017 copied every existing task blocker into question. This also upgrades databases opened
-- by the first draft of PR #57: keep their question boundaries and delivery receipts unchanged.
ALTER TABLE task DROP COLUMN blocked_reason;
