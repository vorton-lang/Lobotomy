//! The manager's records about the projects, in her own database (data-model.md §10.6): how far
//! she has read each project, and the runtime's messages in her inbox about what changed there
//! (manager-actions.md §6 "Manager 什么时候运行").

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::command::{Caller, Command, Cx};
use crate::error::Result;
use crate::task::queue_message;

/// How far the manager has read a project (data-model.md §10.6 跨库操作第 4 条).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    pub project_id: String,
    /// The project's last event the runtime put in her input.
    pub seq: i64,
}

pub fn cursors(conn: &Connection) -> Result<Vec<Cursor>> {
    let mut stmt = conn.prepare("SELECT project_id, seq FROM project_cursor ORDER BY project_id")?;
    let rows = stmt.query_map([], |r| Ok(Cursor { project_id: r.get(0)?, seq: r.get(1)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Puts what the runtime tells the manager about the projects into her inbox and moves her
/// cursors past it, in one transaction of her database (data-model.md §10.6 跨库操作第 5 条).
/// Without a body it only moves the cursors: for a project she sees for the first time.
#[derive(Debug, Serialize, Deserialize)]
pub struct TellManager {
    pub role: String,
    pub body: Option<String>,
    pub cursors: Vec<Cursor>,
}

impl Command for TellManager {
    const NAME: &'static str = "tell_manager";
    type Output = Option<String>;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<Option<String>> {
        caller.require_runtime()?;
        let message = match &self.body {
            Some(body) => Some(queue_message(cx, &self.role, caller, None, body)?),
            None => None,
        };
        for c in &self.cursors {
            cx.tx.execute(
                "INSERT INTO project_cursor (project_id, seq) VALUES (?1, ?2)
                 ON CONFLICT (project_id) DO UPDATE SET seq = excluded.seq",
                params![c.project_id, c.seq],
            )?;
        }
        Ok(message)
    }
}
