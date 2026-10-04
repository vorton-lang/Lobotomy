//! Quota domains (data-model.md §8).
//!
//! A domain is a harness and a login account. Domains belong to the host, outside any project:
//! projects on the same account share the quota (data-model.md §10). They live in their own
//! small database, not in a project's.
//!
//! Recovery is the user's: a rejection blocks the domain, and only the user's retry checks it
//! again. The runtime plans no checks and continues no role on its own.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::error::Result;

/// v1 uses one login per harness (data-model.md §8.1).
pub const DEFAULT_ACCOUNT: &str = "default";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS quota_domain (
  harness    TEXT NOT NULL,
  account    TEXT NOT NULL,
  blocked_at INTEGER,
  resets_at  INTEGER,
  message    TEXT,
  PRIMARY KEY (harness, account)
) STRICT;
";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Domain {
    pub harness: String,
    pub account: String,
    /// When the domain was blocked; `None` when it is open.
    pub blocked_at: Option<i64>,
    /// The reset time the harness reported, in Unix milliseconds. Shown to the user only.
    pub resets_at: Option<i64>,
    pub message: Option<String>,
}

impl Domain {
    fn open(harness: &str) -> Self {
        Self {
            harness: harness.to_owned(),
            account: DEFAULT_ACCOUNT.to_owned(),
            blocked_at: None,
            resets_at: None,
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

    /// A turn was rejected for quota: block the whole domain (data-model.md §8.3). Returns
    /// whether the domain was open before, so the caller notifies once per block.
    pub fn block(&self, harness: &str, resets_at: Option<i64>, message: &str, now: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let was_open = !load(&conn, harness)?.is_blocked();
        conn.execute(
            "INSERT INTO quota_domain (harness, account, blocked_at, resets_at, message) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (harness, account) DO UPDATE SET
               blocked_at = COALESCE(blocked_at, excluded.blocked_at),
               resets_at = excluded.resets_at, message = excluded.message",
            params![harness, DEFAULT_ACCOUNT, now, resets_at, message],
        )?;
        Ok(was_open)
    }

    /// Records the user's check. Passing opens the domain; a rejection keeps it blocked with the
    /// newly reported reset time.
    pub fn record_check(&self, harness: &str, outcome: &CheckOutcome) -> Result<Domain> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        match outcome {
            CheckOutcome::Passed => {
                conn.execute(
                    "DELETE FROM quota_domain WHERE harness = ?1 AND account = ?2",
                    params![harness, DEFAULT_ACCOUNT],
                )?;
            }
            CheckOutcome::Rejected { resets_at } => {
                conn.execute(
                    "UPDATE quota_domain SET resets_at = ?3 WHERE harness = ?1 AND account = ?2",
                    params![harness, DEFAULT_ACCOUNT, resets_at],
                )?;
            }
        }
        load(&conn, harness)
    }
}

fn load(conn: &Connection, harness: &str) -> Result<Domain> {
    let row = conn
        .query_row(
            "SELECT blocked_at, resets_at, message FROM quota_domain WHERE harness = ?1 AND account = ?2",
            params![harness, DEFAULT_ACCOUNT],
            |r| {
                Ok(Domain {
                    harness: harness.to_owned(),
                    account: DEFAULT_ACCOUNT.to_owned(),
                    blocked_at: r.get(0)?,
                    resets_at: r.get(1)?,
                    message: r.get(2)?,
                })
            },
        )
        .optional()?;
    Ok(row.unwrap_or_else(|| Domain::open(harness)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rejection_blocks_only_its_domain_once() {
        let host = HostDb::open_in_memory().unwrap();
        assert!(host.block("claude", Some(100), "limit", 10).unwrap());
        let d = host.domain("claude").unwrap();
        assert_eq!((d.is_blocked(), d.resets_at), (true, Some(100)));
        // A second rejection while blocked is not a new block.
        assert!(!host.block("claude", Some(200), "limit", 11).unwrap());
        assert_eq!(host.domain("claude").unwrap().blocked_at, Some(10));
        assert!(!host.domain("codex").unwrap().is_blocked(), "the other domain stays open");
    }

    #[test]
    fn only_a_passed_check_opens_the_domain() {
        let host = HostDb::open_in_memory().unwrap();
        host.block("claude", Some(100), "limit", 0).unwrap();
        let d = host.record_check("claude", &CheckOutcome::Rejected { resets_at: Some(200) }).unwrap();
        assert_eq!((d.is_blocked(), d.resets_at), (true, Some(200)));
        assert!(!host.record_check("claude", &CheckOutcome::Passed).unwrap().is_blocked());
    }
}
