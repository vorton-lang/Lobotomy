//! Quota domains (data-model.md §8).
//!
//! A domain is a harness and a login account. Domains belong to the host, outside any project:
//! projects on the same account share the quota (data-model.md §10). They live in their own
//! small database, not in a project's.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::error::Result;

/// Checks run this long after the reset time, not at the reset itself (data-model.md §8.4).
pub const CHECK_DELAY_MS: i64 = 60_000;

/// v1 uses one login per harness (data-model.md §8.1).
pub const DEFAULT_ACCOUNT: &str = "default";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS quota_domain (
  harness       TEXT NOT NULL,
  account       TEXT NOT NULL,
  blocked_at    INTEGER,
  resets_at     INTEGER,
  next_check_at INTEGER,
  message       TEXT,
  PRIMARY KEY (harness, account)
) STRICT;
";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Domain {
    pub harness: String,
    pub account: String,
    /// When the domain was blocked; `None` when it is open.
    pub blocked_at: Option<i64>,
    /// The reset time the harness reported, in Unix milliseconds.
    pub resets_at: Option<i64>,
    /// The planned automatic check. `None` while blocked means: wait for the user's retry.
    pub next_check_at: Option<i64>,
    pub message: Option<String>,
}

impl Domain {
    fn open(harness: &str) -> Self {
        Self {
            harness: harness.to_owned(),
            account: DEFAULT_ACCOUNT.to_owned(),
            blocked_at: None,
            resets_at: None,
            next_check_at: None,
            message: None,
        }
    }

    pub fn is_blocked(&self) -> bool {
        self.blocked_at.is_some()
    }
}

/// The result of a quota check call.
#[derive(Clone, Debug, PartialEq)]
pub enum CheckOutcome {
    Passed,
    Rejected { resets_at: Option<i64> },
}

/// The host database: state shared by all projects of this user.
pub struct HostDb {
    conn: Mutex<Connection>,
}

impl HostDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn domain(&self, harness: &str) -> Result<Domain> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        load(&conn, harness)
    }

    /// A turn was rejected for quota: block the whole domain (data-model.md §8.3). With a reset
    /// time, the check is planned for one minute after it; without one, nothing is planned.
    /// Returns whether the domain was open before, so the caller notifies once per block.
    pub fn block(&self, harness: &str, resets_at: Option<i64>, message: &str, now: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let was_open = !load(&conn, harness)?.is_blocked();
        let next = plan(resets_at, now);
        conn.execute(
            "INSERT INTO quota_domain (harness, account, blocked_at, resets_at, next_check_at, message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (harness, account) DO UPDATE SET
               blocked_at = COALESCE(blocked_at, excluded.blocked_at),
               resets_at = excluded.resets_at, next_check_at = excluded.next_check_at, message = excluded.message",
            params![harness, DEFAULT_ACCOUNT, now, resets_at, next, message],
        )?;
        Ok(was_open)
    }

    /// Blocked domains whose planned check is due.
    pub fn due(&self, now: i64) -> Result<Vec<Domain>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let harnesses: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT harness FROM quota_domain WHERE blocked_at IS NOT NULL AND next_check_at <= ?1 ORDER BY harness",
            )?;
            stmt.query_map([now], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        harnesses.iter().map(|h| load(&conn, h)).collect()
    }

    /// The user's retry cancels the planned check; the check result plans the next one
    /// (data-model.md §8.4).
    pub fn cancel_planned_check(&self, harness: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE quota_domain SET next_check_at = NULL WHERE harness = ?1 AND account = ?2",
            params![harness, DEFAULT_ACCOUNT],
        )?;
        Ok(())
    }

    /// Records a check. Passing opens the domain. A rejection with a later reset time plans
    /// the next check after it; otherwise the domain waits for the user's retry, so the runtime
    /// never checks over and over in place.
    pub fn record_check(&self, harness: &str, outcome: &CheckOutcome, now: i64) -> Result<Domain> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        match outcome {
            CheckOutcome::Passed => {
                conn.execute(
                    "DELETE FROM quota_domain WHERE harness = ?1 AND account = ?2",
                    params![harness, DEFAULT_ACCOUNT],
                )?;
            }
            CheckOutcome::Rejected { resets_at } => {
                let resets_at = resets_at.filter(|&t| t > now);
                conn.execute(
                    "UPDATE quota_domain SET resets_at = ?3, next_check_at = ?4 WHERE harness = ?1 AND account = ?2",
                    params![harness, DEFAULT_ACCOUNT, resets_at, plan(resets_at, now)],
                )?;
            }
        }
        load(&conn, harness)
    }
}

fn plan(resets_at: Option<i64>, now: i64) -> Option<i64> {
    resets_at.map(|t| t.max(now) + CHECK_DELAY_MS)
}

fn load(conn: &Connection, harness: &str) -> Result<Domain> {
    let row = conn
        .query_row(
            "SELECT blocked_at, resets_at, next_check_at, message FROM quota_domain WHERE harness = ?1 AND account = ?2",
            params![harness, DEFAULT_ACCOUNT],
            |r| {
                Ok(Domain {
                    harness: harness.to_owned(),
                    account: DEFAULT_ACCOUNT.to_owned(),
                    blocked_at: r.get(0)?,
                    resets_at: r.get(1)?,
                    next_check_at: r.get(2)?,
                    message: r.get(3)?,
                })
            },
        )
        .optional()?;
    Ok(row.unwrap_or_else(|| Domain::open(harness)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    #[test]
    fn a_known_reset_plans_a_check_one_minute_after_it() {
        let host = HostDb::open_in_memory().unwrap();
        assert!(host.block("claude", Some(100 * MIN), "limit", 10 * MIN).unwrap());
        let d = host.domain("claude").unwrap();
        assert!(d.is_blocked());
        assert_eq!(d.next_check_at, Some(101 * MIN));
        assert!(host.due(100 * MIN).unwrap().is_empty(), "not at the reset itself");
        assert_eq!(host.due(101 * MIN).unwrap().len(), 1);
        // A second rejection while blocked is not a new block.
        assert!(!host.block("claude", Some(100 * MIN), "limit", 11 * MIN).unwrap());
        assert!(!host.domain("codex").unwrap().is_blocked(), "the other domain stays open");
    }

    #[test]
    fn an_unknown_reset_waits_for_the_user() {
        let host = HostDb::open_in_memory().unwrap();
        host.block("codex", None, "limit", 0).unwrap();
        assert_eq!(host.domain("codex").unwrap().next_check_at, None);
        assert!(host.due(i64::MAX).unwrap().is_empty());
    }

    #[test]
    fn a_rejected_check_replans_only_with_a_later_reset() {
        let host = HostDb::open_in_memory().unwrap();
        host.block("claude", Some(100 * MIN), "limit", 0).unwrap();
        let d = host.record_check("claude", &CheckOutcome::Rejected { resets_at: Some(200 * MIN) }, 101 * MIN).unwrap();
        assert_eq!(d.next_check_at, Some(201 * MIN));
        // A reset time in the past is no plan: wait for the user instead of checking in place.
        let d = host.record_check("claude", &CheckOutcome::Rejected { resets_at: Some(150 * MIN) }, 201 * MIN).unwrap();
        assert_eq!((d.is_blocked(), d.next_check_at), (true, None));
    }

    #[test]
    fn a_passed_check_opens_the_domain_and_a_retry_cancels_the_plan() {
        let host = HostDb::open_in_memory().unwrap();
        host.block("claude", Some(100 * MIN), "limit", 0).unwrap();
        host.cancel_planned_check("claude").unwrap();
        assert!(host.due(i64::MAX).unwrap().is_empty());
        let d = host.record_check("claude", &CheckOutcome::Passed, 5 * MIN).unwrap();
        assert!(!d.is_blocked());
    }
}
