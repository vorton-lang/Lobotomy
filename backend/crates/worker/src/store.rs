//! The worker host's database, `worker.db`: its tasks, their attempts, the orchestrator's
//! messages and the events reported (worker-interface.md §2, §5, §9). Every event is stored before
//! it is sent, so a subscription can resume from a task's seq.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use lobotomy_core::id::now_ms;
use lobotomy_worker_protocol::event::{
    Artifact, AttemptState, Blocked, Event, EventBody, Evidence, MessageStatus, Question, Receipt, TaskSnapshot,
};
use lobotomy_worker_protocol::ops::{CloseOutcome, TaskSpec};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_environment.sql"),
    include_str!("../migrations/0003_forge.sql"),
];

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
    /// The first of its ports, once its clone is ready.
    pub port_base: Option<u16>,
    /// The instance's attempt (review round) whose delivery the environment has taken.
    pub delivered_round: Option<String>,
    pub pr: Option<(u64, String)>,
    /// CI failures handed back to the agent, and the version of the last one.
    pub ci_rounds: u32,
    pub handed_back: Option<String>,
    /// The artifact version adopted, as `<artifact_id>@<version>`.
    pub adopted: Option<String>,
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

const ATTEMPT_COLUMNS: &str =
    "id, task_id, seq, spec_version, core_task, ending, ended, superseded, port_base, delivered_round, pr_number, pr_url,
     ci_rounds, handed_back, adopted";

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
        port_base: r.get(8)?,
        delivered_round: r.get(9)?,
        pr: match (r.get::<_, Option<i64>>(10)?, r.get::<_, Option<String>>(11)?) {
            (Some(number), Some(url)) => Some((number as u64, url)),
            _ => None,
        },
        ci_rounds: r.get(12)?,
        handed_back: r.get(13)?,
        adopted: r.get(14)?,
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

/// The attempt that runs as the instance's task `core_task`.
pub fn attempt_by_core(c: &Connection, task_id: &str, core_task: &str) -> anyhow::Result<AttemptRow> {
    Ok(c.query_row(
        &format!("SELECT {ATTEMPT_COLUMNS} FROM attempt WHERE task_id = ?1 AND core_task = ?2"),
        [task_id, core_task],
        attempt_row,
    )?)
}

/// The attempt is ready: its clone exists. It gets the lowest port range no running attempt of
/// this worker host has.
pub fn set_prepared(tx: &Transaction<'_>, attempt_id: &str, start: u16, step: u16) -> anyhow::Result<u16> {
    let mut stmt = tx.prepare("SELECT port_base FROM attempt WHERE port_base IS NOT NULL AND ended IS NULL")?;
    let taken: Vec<u16> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let base = (0..).map(|i| start + i * step).find(|b| !taken.contains(b)).context("no port range left")?;
    tx.execute("UPDATE attempt SET port_base = ?2 WHERE id = ?1", params![attempt_id, base])?;
    Ok(base)
}

pub fn set_delivered_round(tx: &Transaction<'_>, attempt_id: &str, round: &str) -> anyhow::Result<()> {
    tx.execute("UPDATE attempt SET delivered_round = ?2 WHERE id = ?1", params![attempt_id, round])?;
    Ok(())
}

/// The latest change an earlier attempt of the task published: what a new one supersedes.
pub fn earlier_change(c: &Connection, task_id: &str, attempt_id: &str) -> anyhow::Result<Option<String>> {
    Ok(c.query_row(
        "SELECT id FROM artifact WHERE task_id = ?1 AND attempt_id <> ?2 AND kind = 'change'
         ORDER BY updated_at DESC LIMIT 1",
        [task_id, attempt_id],
        |r| r.get(0),
    )
    .optional()?)
}

pub fn set_pull_request(tx: &Transaction<'_>, attempt_id: &str, number: u64, url: &str) -> anyhow::Result<()> {
    tx.execute(
        "UPDATE attempt SET pr_number = ?2, pr_url = ?3 WHERE id = ?1",
        params![attempt_id, number as i64, url],
    )?;
    Ok(())
}

/// A CI failure on `version` went back to the agent.
pub fn hand_back(tx: &Transaction<'_>, attempt_id: &str, version: &str) -> anyhow::Result<()> {
    tx.execute(
        "UPDATE attempt SET ci_rounds = ci_rounds + 1, handed_back = ?2 WHERE id = ?1",
        params![attempt_id, version],
    )?;
    Ok(())
}

pub fn set_adopted(tx: &Transaction<'_>, attempt_id: &str, adopted: &str) -> anyhow::Result<()> {
    tx.execute("UPDATE attempt SET adopted = ?2 WHERE id = ?1", params![attempt_id, adopted])?;
    Ok(())
}

/// An artifact's current version, if it was published.
pub fn artifact(c: &Connection, artifact_id: &str) -> anyhow::Result<Option<Artifact>> {
    let current: Option<String> =
        c.query_row("SELECT current FROM artifact WHERE id = ?1", [artifact_id], |r| r.get(0)).optional()?;
    Ok(current.as_deref().map(serde_json::from_str).transpose()?)
}

/// The evidence on one version of an artifact.
pub fn evidence(c: &Connection, artifact_id: &str, version: &str) -> anyhow::Result<Vec<Evidence>> {
    let mut stmt = c.prepare("SELECT current FROM evidence WHERE artifact_id = ?1 AND version = ?2 ORDER BY name")?;
    let rows = stmt.query_map([artifact_id, version], |r| json(&r.get::<_, String>(0)?))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The current version of an artifact, if it was published.
pub fn artifact_version(c: &Connection, artifact_id: &str) -> anyhow::Result<Option<String>> {
    Ok(c.query_row("SELECT json_extract(current, '$.version') FROM artifact WHERE id = ?1", [artifact_id], |r| {
        r.get(0)
    })
    .optional()?)
}

fn artifacts(c: &Connection, attempt_id: &str) -> anyhow::Result<Vec<Artifact>> {
    let mut stmt = c.prepare("SELECT current FROM artifact WHERE attempt_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map([attempt_id], |r| json(&r.get::<_, String>(0)?))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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

/// The attempt's message that replied to `stop`, and when (UTC), if one did.
pub fn stop_reply(c: &Connection, attempt_id: &str, stop: &str) -> anyhow::Result<Option<(String, String)>> {
    Ok(c.query_row(
        "SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at / 1000, 'unixepoch') FROM message
         WHERE attempt_id = ?1 AND json_extract(request, '$.stop_id') = ?2",
        [attempt_id, stop],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?)
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
        EventBody::ArtifactPublished { artifact } => {
            let kind = serde_json::to_value(artifact.kind)?.as_str().unwrap_or_default().to_owned();
            tx.execute(
                "INSERT INTO artifact (id, task_id, attempt_id, kind, current, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (id) DO UPDATE SET current = excluded.current, updated_at = excluded.updated_at",
                params![artifact.artifact_id, task_id, attempt_id, kind, text(artifact), now_ms()],
            )?;
        }
        EventBody::EvidenceUpdated { evidence } => {
            tx.execute(
                "INSERT INTO evidence (artifact_id, version, name, task_id, current, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (artifact_id, version, name) DO UPDATE SET current = excluded.current, updated_at = excluded.updated_at",
                params![evidence.artifact_id, evidence.version, evidence.name, task_id, text(evidence), now_ms()],
            )?;
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
        let artifacts = artifacts(c, &task.attempt_id)?;
        let mut on_them = Vec::new();
        for a in &artifacts {
            on_them.extend(evidence(c, &a.artifact_id, &a.version)?);
        }
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
            artifacts,
            evidence: on_them,
            seq: task.seq,
        });
    }
    Ok(out)
}
