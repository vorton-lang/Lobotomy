//! Quota domains (data-model.md §8).
//!
//! A domain is a harness and a login account. Domains belong to the host, outside any project:
//! projects on the same account share the quota (data-model.md §10). They live in the host
//! database, not in a project's.
//!
//! Recovery is the user's: a rejection blocks the domain, and only the user's retry checks it
//! again. The runtime plans no checks and continues no role on its own.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::error::Result;
use crate::host::HostDb;

/// v1 uses one login per harness (data-model.md §8.1).
pub const DEFAULT_ACCOUNT: &str = "default";

#[derive(Clone, Debug, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
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

impl HostDb {
    pub fn domain(&self, harness: &str) -> Result<Domain> {
        load(&self.conn(), harness)
    }

    /// A turn was rejected for quota: block the whole domain (data-model.md §8.3). Returns
    /// whether the domain was open before, so the caller notifies once per block.
    pub fn block(&self, harness: &str, resets_at: Option<i64>, message: &str, now: i64) -> Result<bool> {
        let conn = self.conn();
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
        let conn = self.conn();
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
