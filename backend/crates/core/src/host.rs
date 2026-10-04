//! The host database: state shared by all projects of this user, outside any project
//! (data-model.md §10). It holds the quota domains (§8) and each harness's permission mode
//! (harness-adapter.md §1.9).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;

/// Applied like a project's migrations (`db.rs`): numbered, each once, in its own transaction.
const MIGRATIONS: &[&str] = &[include_str!("../host_migrations/0001_init.sql")];

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

    fn init(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        crate::db::migrate(&mut conn, MIGRATIONS)?;
        conn.pragma_update(None, "foreign_keys", true)?;
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

    /// Host databases made before migrations existed have the tables and no version (#16).
    #[test]
    fn a_host_database_from_before_migrations_keeps_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.execute("INSERT INTO harness_setting VALUES ('codex', 'auto_review', 1)", []).unwrap();
        }
        let host = HostDb::open(&path).unwrap();
        assert_eq!(host.permission("codex").unwrap().as_deref(), Some("auto_review"));
        let version: i64 = host.conn().pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

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
