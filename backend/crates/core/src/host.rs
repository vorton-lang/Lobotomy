//! The host database: state shared by all projects of this user, outside any project
//! (data-model.md §10). It holds the quota domains (§8) and each harness's permission mode
//! (harness-adapter.md §1.9).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;

/// One row per harness whose setting the user changed; a harness without a row uses the default.
/// The values are the adapter's (`lobotomy_harness::Permission`), which checks them.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS harness_setting (
  harness    TEXT PRIMARY KEY,
  permission TEXT NOT NULL,
  changed_at INTEGER NOT NULL
) STRICT;
";

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
        conn.execute_batch(crate::quota::SCHEMA)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The harness's permission mode, if the user set one.
    pub fn permission(&self, harness: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT permission FROM harness_setting WHERE harness = ?1", [harness], |r| r.get(0))
            .optional()?)
    }

    pub fn set_permission(&self, harness: &str, permission: &str, now: i64) -> Result<()> {
        self.conn().execute(
            "INSERT INTO harness_setting (harness, permission, changed_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (harness) DO UPDATE SET permission = excluded.permission, changed_at = excluded.changed_at",
            params![harness, permission, now],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_harness_keeps_its_own_permission() {
        let host = HostDb::open_in_memory().unwrap();
        assert_eq!(host.permission("codex").unwrap(), None);
        host.set_permission("codex", "auto_review", 1).unwrap();
        assert_eq!(host.permission("codex").unwrap().as_deref(), Some("auto_review"));
        assert_eq!(host.permission("claude").unwrap(), None, "the other harness keeps its default");
        host.set_permission("codex", "full", 2).unwrap();
        assert_eq!(host.permission("codex").unwrap().as_deref(), Some("full"));
    }
}
