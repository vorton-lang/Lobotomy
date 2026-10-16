//! A task's detail: its record, criteria versions, attempts and decisions (frontend.md §7;
//! data-model.md §9.1).

use rusqlite::{Connection, Row};
use serde::Serialize;
use serde_json::Value;

use crate::error::Result;
use crate::task::{Attempt, Message, Task, list_attempts, load_task, undelivered_user_messages};

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct TaskDetail {
    pub task: Task,
    pub criteria: Vec<Criteria>,
    pub attempts: Vec<Attempt>,
    pub decisions: Vec<Decision>,
    /// The user's messages to the task that its executor never got (data-model.md §4.2).
    pub undelivered_messages: Vec<Message>,
}

/// One version of the completion criteria (data-model.md §4.4).
#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct Criteria {
    pub version: i64,
    pub text: String,
    pub created_by: String,
    pub created_at: i64,
}

/// A decision about the task: who made it, and the details it records.
#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct Decision {
    pub id: String,
    pub kind: String,
    pub actor: String,
    pub detail: Value,
    pub created_at: i64,
}

fn rows<T>(conn: &Connection, sql: &str, task_id: &str, row: impl FnMut(&Row<'_>) -> Result<T>) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    stmt.query_and_then([task_id], row)?.collect()
}

pub fn task_detail(conn: &Connection, task_id: &str) -> Result<TaskDetail> {
    let task = load_task(conn, task_id)?;
    let criteria = rows(
        conn,
        "SELECT version, text, created_by, created_at FROM criteria_version WHERE task_id = ?1 ORDER BY version",
        task_id,
        |r| Ok(Criteria { version: r.get(0)?, text: r.get(1)?, created_by: r.get(2)?, created_at: r.get(3)? }),
    )?;
    let decisions = rows(
        conn,
        "SELECT id, kind, actor, detail, created_at FROM decision WHERE task_id = ?1 ORDER BY created_at, id",
        task_id,
        |r| {
            Ok(Decision {
                id: r.get(0)?,
                kind: r.get(1)?,
                actor: r.get(2)?,
                detail: serde_json::from_str(&r.get::<_, String>(3)?)?,
                created_at: r.get(4)?,
            })
        },
    )?;
    Ok(TaskDetail {
        criteria,
        attempts: list_attempts(conn, task_id)?,
        decisions,
        undelivered_messages: undelivered_user_messages(conn, task_id)?,
        task,
    })
}
