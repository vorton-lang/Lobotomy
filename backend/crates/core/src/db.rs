use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms};

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_turns.sql"),
    include_str!("../migrations/0003_session_per_task.sql"),
    include_str!("../migrations/0004_results.sql"),
    include_str!("../migrations/0005_workspace_work.sql"),
    include_str!("../migrations/0006_task_boundaries.sql"),
    include_str!("../migrations/0007_attempt_conflicts.sql"),
    include_str!("../migrations/0008_attempt_done_summary.sql"),
    include_str!("../migrations/0009_attempt_trial.sql"),
    include_str!("../migrations/0010_event_command.sql"),
];

/// One project's database.
///
/// Every write goes through `execute`, which holds the only write connection, so command
/// transactions never interleave (notes/architecture.md §3). Reads have a read-only connection of
/// their own: with WAL, a read never waits for a write transaction, nor a write for a read (#16).
pub struct Db {
    conn: Mutex<Connection>,
    /// `None` for a database in memory, which a second connection cannot open; reads then use the
    /// write connection.
    reader: Option<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        let mut db = Self::init(conn)?;
        // Opened after the migrations, so it never sees a schema in between.
        let reader =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        reader.busy_timeout(Duration::from_secs(5))?;
        db.reader = Some(Mutex::new(reader));
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        migrate(&mut conn, MIGRATIONS)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Ok(Self { conn: Mutex::new(conn), reader: None })
    }

    /// Runs a command in one transaction and records it in the command log.
    pub fn execute<C: Command>(&self, caller: &Caller, cmd: &C) -> Result<C::Output> {
        self.execute_recorded(caller, cmd).map(|(out, _)| out)
    }

    /// Like `execute`, and also returns the id of the command record. A repeated command returns
    /// the id of the first record.
    pub fn execute_recorded<C: Command>(&self, caller: &Caller, cmd: &C) -> Result<(C::Output, String)> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scope = caller.scope();

        if let Some(key) = cmd.idem_key() {
            let prior: Option<(String, String, String, String)> = tx
                .query_row(
                    "SELECT id, name, args, result FROM command_record WHERE caller = ?1 AND idem_key = ?2",
                    params![scope, key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            if let Some((id, name, args, result)) = prior {
                // A retry must be the same command with the same arguments. Anything else that
                // reuses the key is rejected, so it can neither run nor receive another
                // command's result.
                if name != C::NAME {
                    return Err(Error::rejected(
                        "idempotency_key_reused",
                        format!("key {key} was already used by {name}"),
                    ));
                }
                if serde_json::from_str::<serde_json::Value>(&args)? != serde_json::to_value(cmd)? {
                    return Err(Error::rejected(
                        "idempotency_key_reused",
                        format!("key {key} was already used by {name} with other arguments"),
                    ));
                }
                return Ok((serde_json::from_str(&result)?, id));
            }
        }

        let now = now_ms();
        // The id comes first so that the command's events can name it. The record itself is
        // written after `apply`, with the result, so the events come before it: their reference
        // to the record is checked at commit (migration 0010), not at each insert. That keeps the
        // record one insert, never a row with a result still to come.
        let id = new_id("cmd");
        let out = cmd.apply(caller, &mut Cx::new(&tx, now, &id))?;
        tx.execute(
            "INSERT INTO command_record (id, name, caller, idem_key, args, result, created_at, turn_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                C::NAME,
                scope,
                cmd.idem_key(),
                serde_json::to_string(cmd)?,
                serde_json::to_string(&out)?,
                now,
                caller.turn_id()
            ],
        )?;
        tx.commit()?;
        Ok((out, id))
    }

    /// Writes records that are not business state, such as transcript items, in one
    /// transaction without a command record (data-model.md §7.2).
    pub fn write<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Runs read-only queries in one read transaction: all of them see the same committed state,
    /// although commands commit meanwhile.
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let Some(reader) = &self.reader else {
            return f(&self.conn.lock().unwrap_or_else(|e| e.into_inner()));
        };
        let mut conn = reader.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.transaction()?;
        f(&tx)
    }
}

/// Applies the migrations not yet applied, each in its own transaction.
///
/// Foreign keys are off while they run and checked before each commit. A migration may then
/// rebuild a table other tables point to, as SQLite's "twelve steps" require for changing a CHECK
/// constraint: with foreign keys on, dropping the old table would fail (#16). SQLite ignores the
/// pragma inside a transaction, so a migration cannot switch it itself.
///
/// A database a newer backend has migrated further is refused, not used: this backend does not
/// know what the newer tables mean. A project's then fails to open alone (data-model.md §10.3).
pub(crate) fn migrate(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    let applied = conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize;
    if applied > migrations.len() {
        return Err(Error::rejected(
            "newer_database",
            format!("数据库是更新版本的 Lobotomy 写的（第 {applied} 版，这个后端只认到第 {} 版）", migrations.len()),
        ));
    }
    conn.pragma_update(None, "foreign_keys", false)?;
    for (i, sql) in migrations.iter().enumerate().skip(applied) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        let dangling: Option<String> = tx.query_row("PRAGMA foreign_key_check", [], |r| r.get(0)).optional()?;
        if let Some(table) = dangling {
            return Err(Error::invariant(format!("migration {} leaves rows of {table} pointing at nothing", i + 1)));
        }
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARENT_AND_CHILD: &str = "
        CREATE TABLE parent (id TEXT PRIMARY KEY, kind TEXT CHECK (kind IN ('a'))) STRICT;
        CREATE TABLE child (id TEXT PRIMARY KEY, parent TEXT NOT NULL REFERENCES parent (id)) STRICT;
        INSERT INTO parent VALUES ('p', 'a');
        INSERT INTO child VALUES ('c', 'p');";

    /// Widening a CHECK constraint means rebuilding the table; other tables keep pointing at it.
    const WIDEN_PARENT: &str = "
        CREATE TABLE parent_new (id TEXT PRIMARY KEY, kind TEXT CHECK (kind IN ('a', 'b'))) STRICT;
        INSERT INTO parent_new SELECT * FROM parent;
        DROP TABLE parent;
        ALTER TABLE parent_new RENAME TO parent;";

    #[test]
    fn a_migration_can_rebuild_a_table_others_point_to() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, &[PARENT_AND_CHILD, WIDEN_PARENT]).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute("INSERT INTO parent VALUES ('q', 'b')", []).unwrap();
        let orphan = conn.execute("INSERT INTO child VALUES ('d', 'nowhere')", []);
        assert!(orphan.is_err(), "the reference still holds after the rebuild");
    }

    #[test]
    fn a_migration_that_leaves_dangling_rows_is_not_applied() {
        let mut conn = Connection::open_in_memory().unwrap();
        let broken = "DELETE FROM parent;";
        let err = migrate(&mut conn, &[PARENT_AND_CHILD, broken]).unwrap_err();
        assert!(matches!(err, Error::Invariant(_)), "{err}");
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, 1, "the broken migration rolled back");
        let parents: i64 = conn.query_row("SELECT COUNT(*) FROM parent", [], |r| r.get(0)).unwrap();
        assert_eq!(parents, 1);
    }

    #[test]
    fn a_database_from_a_newer_backend_is_refused_untouched() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, &[PARENT_AND_CHILD, WIDEN_PARENT]).unwrap();
        let err = migrate(&mut conn, &[PARENT_AND_CHILD]).unwrap_err();
        assert_eq!(err.code(), Some("newer_database"), "{err}");
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn events_from_before_they_named_their_command_are_kept_and_still_read() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Up to 0009: events name no command yet.
        migrate(&mut conn, &MIGRATIONS[..9]).unwrap();
        conn.execute(
            "INSERT INTO event (kind, entity, payload, created_at) VALUES ('task.created', 'task_old', '{}', 1)",
            [],
        )
        .unwrap();
        migrate(&mut conn, MIGRATIONS).unwrap();
        let events = crate::view::events_after(&conn, 0).unwrap();
        let [old] = events.as_slice() else { panic!("expected the one old event, got {}", events.len()) };
        assert_eq!((old.seq, old.kind.as_str(), old.entity.as_str()), (1, "task.created", "task_old"));
        assert_eq!(
            (old.command_id.as_deref(), old.caller.as_deref(), old.caller_turn_id.as_deref()),
            (None, None, None)
        );
    }

    #[test]
    fn an_event_naming_an_unrecorded_command_does_not_commit() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, MIGRATIONS).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "INSERT INTO event (kind, entity, payload, created_at, command_id)
             VALUES ('task.created', 'task_x', '{}', 1, 'cmd_nowhere')",
            [],
        )
        .expect("the reference is checked at commit, not at the insert");
        assert!(tx.commit().is_err());
        let events: i64 = conn.query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0)).unwrap();
        assert_eq!(events, 0);
    }

    /// A database in a file, as a project's is; one in memory has no read connection.
    fn file_db() -> (tempfile::TempDir, std::sync::Arc<Db>) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("lobotomy.db")).unwrap();
        (dir, std::sync::Arc::new(db))
    }

    fn roles(conn: &Connection) -> Result<i64> {
        Ok(conn.query_row("SELECT COUNT(*) FROM role", [], |r| r.get(0))?)
    }

    fn add_role(tx: &Transaction<'_>) -> Result<()> {
        tx.execute("INSERT INTO role (name, kind, harness) VALUES ('Yesod', 'worker', 'codex')", [])?;
        Ok(())
    }

    /// Runs `f` on another thread; `None` when it has not returned within five seconds.
    fn within_seconds<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || done.send(f()));
        result.recv_timeout(Duration::from_secs(5)).ok()
    }

    #[test]
    fn a_read_does_not_wait_for_a_write_in_progress() {
        let (_dir, db) = file_db();
        db.write(|tx| {
            add_role(tx)?;
            let other = db.clone();
            let seen = within_seconds(move || other.read(roles).unwrap());
            assert_eq!(seen, Some(1), "the read sees the last commit, not the write in progress");
            Ok(())
        })
        .unwrap();
        assert_eq!(db.read(roles).unwrap(), 2);
    }

    #[test]
    fn a_read_sees_one_state_while_others_commit() {
        let (_dir, db) = file_db();
        db.read(|conn| {
            let before = roles(conn)?;
            let other = db.clone();
            let committed = within_seconds(move || other.write(add_role).is_ok());
            assert_eq!(committed, Some(true), "a write does not wait for a read");
            assert_eq!(roles(conn)?, before, "the read goes on with the state it started from");
            Ok(())
        })
        .unwrap();
        assert_eq!(db.read(roles).unwrap(), 2);
    }

    #[test]
    fn the_read_connection_cannot_write() {
        let (_dir, db) = file_db();
        assert!(db.read(|conn| Ok(conn.execute("DELETE FROM role", [])?)).is_err());
        assert_eq!(db.read(roles).unwrap(), 1);
    }
}
