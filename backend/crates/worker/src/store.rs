//! The worker host's database, `worker.db`: its tasks, their attempts, the orchestrator's
//! messages and the events reported (worker-interface.md §2, §5, §9). Every event is stored before
//! it is sent, so a subscription can resume from a task's seq.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use lobotomy_core::id::now_ms;
use lobotomy_worker_protocol::event::{
    AttemptState, Blocked, Event, EventBody, MessageStatus, Question, Receipt, TaskSnapshot,
};
use lobotomy_worker_protocol::ops::{CloseOutcome, TaskSpec};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

pub struct Store {
    conn: Mutex<Connection>,
}

/// An attempt's state as the orchestrator is told it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub state: AttemptState,
    pub blocked: Option<Blocked>,
    pub reason: Option<String>,
}

impl Status {
    pub fn of(state: AttemptState) -> Self {
        Self { state, blocked: None, reason: None }
    }

    pub fn event(&self) -> EventBody {
        EventBody::StateChanged { state: self.state, blocked: self.blocked.clone(), reason: self.reason.clone() }
    }
}

#[derive(Clone, Debug)]
pub struct TaskRow {
    pub id: String,
    pub request: String,
    pub spec_version: u32,
    pub spec: TaskSpec,
    pub attempt_id: String,
    pub status: Status,
    pub question: Option<Question>,
    pub closed: Option<CloseOutcome>,
    pub seq: u64,
    pub core_seq: i64,
}

#[derive(Clone, Debug)]
pub struct AttemptRow {
    pub id: String,
    pub task_id: String,
    pub seq: u32,
    pub spec_version: u32,
    pub core_task: String,
    /// The end asked for, `canceled` or `abandoned`, until it is reached.
    pub ending: Option<AttemptState>,
    pub ended: Option<Status>,
    pub superseded: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct MessageRow {
    pub id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub request: String,
    pub core_id: String,
    pub receipt: Receipt,
}

fn json<T: serde::de::DeserializeOwned>(text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))
}

fn text(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("stored values are JSON serializable")
}

fn close_outcome(text: Option<String>) -> Option<CloseOutcome> {
    match text.as_deref() {
        Some("adopted") => Some(CloseOutcome::Adopted),
        Some("abandoned") => Some(CloseOutcome::Abandoned),
        _ => None,
    }
}

pub fn close_text(outcome: CloseOutcome) -> &'static str {
    match outcome {
        CloseOutcome::Adopted => "adopted",
        CloseOutcome::Abandoned => "abandoned",
    }
}

fn ending_text(state: AttemptState) -> &'static str {
    match state {
        AttemptState::Abandoned => "abandoned",
        _ => "canceled",
    }
}

const TASK_COLUMNS: &str = "id, request, spec_version, spec, attempt_id, status, question, closed, seq, core_seq";

fn task_row(r: &Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        id: r.get(0)?,
        request: r.get(1)?,
        spec_version: r.get(2)?,
        spec: json(&r.get::<_, String>(3)?)?,
        attempt_id: r.get(4)?,
        status: json(&r.get::<_, String>(5)?)?,
        question: r.get::<_, Option<String>>(6)?.as_deref().map(json).transpose()?,
        closed: close_outcome(r.get(7)?),
        seq: r.get::<_, i64>(8)? as u64,
        core_seq: r.get(9)?,
    })
}

const ATTEMPT_COLUMNS: &str = "id, task_id, seq, spec_version, core_task, ending, ended, superseded";

fn attempt_row(r: &Row<'_>) -> rusqlite::Result<AttemptRow> {
    Ok(AttemptRow {
        id: r.get(0)?,
        task_id: r.get(1)?,
        seq: r.get(2)?,
        spec_version: r.get(3)?,
        core_task: r.get(4)?,
        ending: match r.get::<_, Option<String>>(5)?.as_deref() {
            Some("abandoned") => Some(AttemptState::Abandoned),
            Some(_) => Some(AttemptState::Canceled),
            None => None,
        },
        ended: r.get::<_, Option<String>>(6)?.as_deref().map(json).transpose()?,
        superseded: json(&r.get::<_, String>(7)?)?,
    })
}

const MESSAGE_COLUMNS: &str = "id, task_id, attempt_id, request, core_id, receipt";

fn message_row(r: &Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        id: r.get(0)?,
        task_id: r.get(1)?,
        attempt_id: r.get(2)?,
        request: r.get(3)?,
        core_id: r.get(4)?,
        receipt: json(&r.get::<_, String>(5)?)?,
    })
}

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let mut conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        lobotomy_core::db::migrate(&mut conn, MIGRATIONS)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Runs `f` in one write transaction.
    pub fn write<T>(&self, f: impl FnOnce(&Transaction<'_>) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> anyhow::Result<T>) -> anyhow::Result<T> {
        f(&self.conn.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

pub fn task(c: &Connection, id: &str) -> anyhow::Result<Option<TaskRow>> {
    Ok(c.query_row(&format!("SELECT {TASK_COLUMNS} FROM task WHERE id = ?1"), [id], task_row).optional()?)
}

pub fn tasks(c: &Connection) -> anyhow::Result<Vec<TaskRow>> {
    let mut stmt = c.prepare(&format!("SELECT {TASK_COLUMNS} FROM task ORDER BY created_at, id"))?;
    Ok(stmt.query_map([], task_row)?.collect::<rusqlite::Result<_>>()?)
}

pub fn attempt(c: &Connection, id: &str) -> anyhow::Result<AttemptRow> {
    Ok(c.query_row(&format!("SELECT {ATTEMPT_COLUMNS} FROM attempt WHERE id = ?1"), [id], attempt_row)?)
}

pub fn attempt_for_spec(c: &Connection, task_id: &str, spec_version: u32) -> anyhow::Result<Option<AttemptRow>> {
    Ok(c.query_row(
        &format!("SELECT {ATTEMPT_COLUMNS} FROM attempt WHERE task_id = ?1 AND spec_version = ?2"),
        params![task_id, spec_version],
        attempt_row,
    )
    .optional()?)
}

/// The task's attempts asked to end that have not yet.
pub fn ending_attempts(c: &Connection, task_id: &str) -> anyhow::Result<Vec<AttemptRow>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {ATTEMPT_COLUMNS} FROM attempt WHERE task_id = ?1 AND ending IS NOT NULL AND ended IS NULL ORDER BY seq"
    ))?;
    Ok(stmt.query_map([task_id], attempt_row)?.collect::<rusqlite::Result<_>>()?)
}

pub fn message(c: &Connection, id: &str) -> anyhow::Result<Option<MessageRow>> {
    Ok(c.query_row(&format!("SELECT {MESSAGE_COLUMNS} FROM message WHERE id = ?1"), [id], message_row).optional()?)
}

pub fn attempt_messages(c: &Connection, attempt_id: &str) -> anyhow::Result<Vec<MessageRow>> {
    let mut stmt =
        c.prepare(&format!("SELECT {MESSAGE_COLUMNS} FROM message WHERE attempt_id = ?1 ORDER BY created_at, id"))?;
    Ok(stmt.query_map([attempt_id], message_row)?.collect::<rusqlite::Result<_>>()?)
}

pub struct NewTask<'a> {
    pub id: &'a str,
    pub request: &'a str,
    pub spec_version: u32,
    pub spec: &'a TaskSpec,
    pub attempt_id: &'a str,
    pub core_task: &'a str,
}

/// A new task with attempt 1, submitted.
pub fn insert_task(tx: &Transaction<'_>, new: &NewTask<'_>) -> anyhow::Result<()> {
    let now = now_ms();
    tx.execute(
        "INSERT INTO task (id, request, spec_version, spec, attempt_id, status, seq, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
        params![
            new.id,
            new.request,
            new.spec_version,
            text(new.spec),
            new.attempt_id,
            text(&Status::of(AttemptState::Submitted)),
            now
        ],
    )?;
    insert_attempt(tx, new.id, new.attempt_id, new.spec_version, new.core_task, &[])
}

/// A new attempt, made the task's current one by the caller.
pub fn insert_attempt(
    tx: &Transaction<'_>,
    task_id: &str,
    attempt_id: &str,
    spec_version: u32,
    core_task: &str,
    superseded: &[String],
) -> anyhow::Result<()> {
    tx.execute(
        "INSERT INTO attempt (id, task_id, seq, spec_version, core_task, superseded, started_at)
         VALUES (?1, ?2, (SELECT COALESCE(MAX(seq), 0) + 1 FROM attempt WHERE task_id = ?2), ?3, ?4, ?5, ?6)",
        params![attempt_id, task_id, spec_version, core_task, text(&superseded), now_ms()],
    )?;
    Ok(())
}

/// Makes `attempt_id` the task's current attempt, on a new spec.
pub fn set_current(
    tx: &Transaction<'_>,
    task_id: &str,
    attempt_id: &str,
    spec_version: u32,
    spec: &TaskSpec,
) -> anyhow::Result<()> {
    tx.execute(
        "UPDATE task SET attempt_id = ?2, spec_version = ?3, spec = ?4, status = ?5, question = NULL WHERE id = ?1",
        params![task_id, attempt_id, spec_version, text(spec), text(&Status::of(AttemptState::Submitted))],
    )?;
    Ok(())
}

pub fn set_ending(tx: &Transaction<'_>, attempt_id: &str, ending: AttemptState) -> anyhow::Result<()> {
    tx.execute(
        "UPDATE attempt SET ending = ?2 WHERE id = ?1 AND ending IS NULL AND ended IS NULL",
        params![attempt_id, ending_text(ending)],
    )?;
    Ok(())
}

pub fn set_ended(tx: &Transaction<'_>, attempt_id: &str, status: &Status) -> anyhow::Result<()> {
    tx.execute("UPDATE attempt SET ended = ?2 WHERE id = ?1", params![attempt_id, text(status)])?;
    Ok(())
}

pub fn set_closed(tx: &Transaction<'_>, task_id: &str, outcome: CloseOutcome) -> anyhow::Result<()> {
    tx.execute("UPDATE task SET closed = ?2 WHERE id = ?1", params![task_id, close_text(outcome)])?;
    Ok(())
}

pub fn set_core_seq(tx: &Transaction<'_>, task_id: &str, seq: i64) -> anyhow::Result<()> {
    tx.execute("UPDATE task SET core_seq = ?2 WHERE id = ?1", params![task_id, seq])?;
    Ok(())
}

pub fn insert_message(tx: &Transaction<'_>, row: &MessageRow) -> anyhow::Result<()> {
    tx.execute(
        "INSERT INTO message (id, task_id, attempt_id, request, core_id, receipt, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![row.id, row.task_id, row.attempt_id, row.request, row.core_id, text(&row.receipt), now_ms()],
    )?;
    Ok(())
}

/// Stores an event of the task with the next seq, and with it what it tells: a state, an open
/// question or a receipt becomes what the orchestrator was last told.
pub fn append(tx: &Transaction<'_>, task_id: &str, attempt_id: &str, body: EventBody) -> anyhow::Result<Event> {
    let current: String = tx.query_row("SELECT attempt_id FROM task WHERE id = ?1", [task_id], |r| r.get(0))?;
    match &body {
        EventBody::StateChanged { state, blocked, reason } if current == attempt_id => {
            let status = Status { state: *state, blocked: blocked.clone(), reason: reason.clone() };
            tx.execute("UPDATE task SET status = ?2 WHERE id = ?1", params![task_id, text(&status)])?;
        }
        EventBody::QuestionOpened { question } if current == attempt_id => {
            tx.execute("UPDATE task SET question = ?2 WHERE id = ?1", params![task_id, text(question)])?;
        }
        EventBody::QuestionClosed { .. } if current == attempt_id => {
            tx.execute("UPDATE task SET question = NULL WHERE id = ?1", [task_id])?;
        }
        EventBody::MessageReceipt { message_id, receipt } => {
            tx.execute("UPDATE message SET receipt = ?2 WHERE id = ?1", params![message_id, text(receipt)])?;
        }
        _ => {}
    }
    let seq: i64 =
        tx.query_row("UPDATE task SET seq = seq + 1 WHERE id = ?1 RETURNING seq", [task_id], |r| r.get(0))?;
    tx.execute(
        "INSERT INTO event (task_id, seq, attempt_id, body, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![task_id, seq, attempt_id, text(&body), now_ms()],
    )?;
    Ok(Event { task_id: task_id.to_owned(), attempt_id: attempt_id.to_owned(), seq: seq as u64, body })
}

pub fn events_after(c: &Connection, task_id: &str, after: u64) -> anyhow::Result<Vec<Event>> {
    let mut stmt = c.prepare("SELECT seq, attempt_id, body FROM event WHERE task_id = ?1 AND seq > ?2 ORDER BY seq")?;
    let rows = stmt.query_map(params![task_id, after as i64], |r| {
        Ok(Event {
            task_id: task_id.to_owned(),
            seq: r.get::<_, i64>(0)? as u64,
            attempt_id: r.get(1)?,
            body: json(&r.get::<_, String>(2)?)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every task as it stands (§9).
pub fn snapshot(c: &Connection) -> anyhow::Result<Vec<TaskSnapshot>> {
    let mut out = Vec::new();
    for task in tasks(c)? {
        let messages = attempt_messages(c, &task.attempt_id)?
            .into_iter()
            .map(|m| MessageStatus { message_id: m.id, receipt: m.receipt })
            .collect();
        out.push(TaskSnapshot {
            task_id: task.id,
            spec_version: task.spec_version,
            attempt_id: task.attempt_id,
            state: task.status.state,
            blocked: task.status.blocked,
            reason: task.status.reason,
            closed: task.closed,
            question: task.question,
            messages,
            artifacts: Vec::new(),
            evidence: Vec::new(),
            seq: task.seq,
        });
    }
    Ok(out)
}
