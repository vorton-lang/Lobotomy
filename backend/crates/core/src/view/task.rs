//! A task's detail: its record, criteria versions, attempts, captures, verifications with their
//! checks, decisions and publications (frontend.md §7; data-model.md §9.1).

use rusqlite::{Connection, Row};
use serde::Serialize;
use serde_json::Value;

use crate::capture::{Capture, task_captures};
use crate::error::Result;
use crate::task::{Attempt, Message, Task, list_attempts, load_task, undelivered_user_messages};
use crate::verify::{Verification, task_verifications};

#[derive(Serialize)]
pub struct TaskDetail {
    pub task: Task,
    pub criteria: Vec<Criteria>,
    pub attempts: Vec<Attempt>,
    pub captures: Vec<Capture>,
    pub verifications: Vec<Verification>,
    /// The checks of all its verifications, in the order they ran.
    pub checks: Vec<CheckRun>,
    pub decisions: Vec<Decision>,
    pub publications: Vec<Publication>,
    /// The user's messages to the task that its executor never got (data-model.md §4.2).
    pub undelivered_messages: Vec<Message>,
}

/// One version of the completion criteria (data-model.md §4.4).
#[derive(Serialize)]
pub struct Criteria {
    pub version: i64,
    pub text: String,
    pub created_by: String,
    pub created_at: i64,
}

/// One check command of a verification and how it ran (harness-adapter.md §4.2).
#[derive(Serialize)]
pub struct CheckRun {
    pub id: String,
    pub verification_id: String,
    pub seq: i64,
    pub command: String,
    pub exit_code: Option<i64>,
    pub timed_out: bool,
    /// `stdout` and `stderr`, each text or a reference to the blob store.
    pub output: Value,
    pub duration_ms: i64,
}

/// A decision about the task: who made it, and the details it records.
#[derive(Serialize)]
pub struct Decision {
    pub id: String,
    pub kind: String,
    pub actor: String,
    pub detail: Value,
    pub created_at: i64,
}

/// An integration version the task's acceptance published.
#[derive(Serialize)]
pub struct Publication {
    pub rev: i64,
    pub commit_id: String,
    /// The integration version before it.
    pub previous: String,
    pub created_at: i64,
}

fn rows<T>(
    conn: &Connection,
    sql: &str,
    task_id: &str,
    row: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([task_id], row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn task_detail(conn: &Connection, task_id: &str) -> Result<TaskDetail> {
    let task = load_task(conn, task_id)?;
    let criteria = rows(
        conn,
        "SELECT version, text, created_by, created_at FROM criteria_version WHERE task_id = ?1 ORDER BY version",
        task_id,
        |r| Ok(Criteria { version: r.get(0)?, text: r.get(1)?, created_by: r.get(2)?, created_at: r.get(3)? }),
    )?;
    let checks = rows(
        conn,
        "SELECT c.id, c.verification_id, c.seq, c.command, c.exit_code, c.timed_out, c.output, c.duration_ms
         FROM check_run c JOIN verification v ON v.id = c.verification_id WHERE v.task_id = ?1
         ORDER BY v.created_at, c.seq",
        task_id,
        |r| {
            let check = CheckRun {
                id: r.get(0)?,
                verification_id: r.get(1)?,
                seq: r.get(2)?,
                command: r.get(3)?,
                exit_code: r.get(4)?,
                timed_out: r.get(5)?,
                output: Value::Null,
                duration_ms: r.get(7)?,
            };
            Ok((check, r.get::<_, String>(6)?))
        },
    )?
    .into_iter()
    .map(|(mut check, output)| {
        check.output = serde_json::from_str(&output)?;
        Ok(check)
    })
    .collect::<Result<_>>()?;
    let decisions = rows(
        conn,
        "SELECT id, kind, actor, detail, created_at FROM decision WHERE task_id = ?1 ORDER BY created_at, id",
        task_id,
        |r| {
            let decision = Decision {
                id: r.get(0)?,
                kind: r.get(1)?,
                actor: r.get(2)?,
                detail: Value::Null,
                created_at: r.get(4)?,
            };
            Ok((decision, r.get::<_, String>(3)?))
        },
    )?
    .into_iter()
    .map(|(mut decision, detail)| {
        decision.detail = serde_json::from_str(&detail)?;
        Ok(decision)
    })
    .collect::<Result<_>>()?;
    let publications = rows(
        conn,
        "SELECT rev, commit_id, previous, created_at FROM publication WHERE task_id = ?1 ORDER BY rev",
        task_id,
        |r| Ok(Publication { rev: r.get(0)?, commit_id: r.get(1)?, previous: r.get(2)?, created_at: r.get(3)? }),
    )?;
    Ok(TaskDetail {
        criteria,
        attempts: list_attempts(conn, task_id)?,
        captures: task_captures(conn, task_id)?,
        verifications: task_verifications(conn, task_id)?,
        checks,
        decisions,
        publications,
        undelivered_messages: undelivered_user_messages(conn, task_id)?,
        task,
    })
}
