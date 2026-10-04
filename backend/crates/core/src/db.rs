use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms};

const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

/// One project's database.
///
/// Every write goes through `execute`, which holds the only write connection, so command
/// transactions never interleave (notes/m1-plan.md §2).
pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        migrate(&mut conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Runs a command in one transaction and records it in the command log.
    pub fn execute<C: Command>(&self, caller: &Caller, cmd: &C) -> Result<C::Output> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scope = caller.scope();

        if let Some(key) = cmd.idem_key() {
            let prior: Option<(String, String, String)> = tx
                .query_row(
                    "SELECT name, args, result FROM command_record WHERE caller = ?1 AND idem_key = ?2",
                    params![scope, key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            if let Some((name, args, result)) = prior {
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
                return Ok(serde_json::from_str(&result)?);
            }
        }

        let now = now_ms();
        let out = cmd.apply(caller, &mut Cx::new(&tx, now))?;
        tx.execute(
            "INSERT INTO command_record (id, name, caller, idem_key, args, result, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                new_id("cmd"),
                C::NAME,
                scope,
                cmd.idem_key(),
                serde_json::to_string(cmd)?,
                serde_json::to_string(&out)?,
                now
            ],
        )?;
        tx.commit()?;
        Ok(out)
    }

    /// Runs a read-only query on the write connection.
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&conn)
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let applied = conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}
