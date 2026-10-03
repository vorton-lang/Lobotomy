//! Tasks, completion criteria, attempts, task-level occupancy and queued messages
//! (data-model.md §2, §3.4, §4; roles-and-tasks.md §2).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::id::new_id;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Queued,
    Executing,
    Verifying,
    Accepting,
    Done,
    Abandoned,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Queued => "queued",
            Phase::Executing => "executing",
            Phase::Verifying => "verifying",
            Phase::Accepting => "accepting",
            Phase::Done => "done",
            Phase::Abandoned => "abandoned",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "queued" => Phase::Queued,
            "executing" => Phase::Executing,
            "verifying" => Phase::Verifying,
            "accepting" => Phase::Accepting,
            "done" => Phase::Done,
            "abandoned" => Phase::Abandoned,
            other => return Err(Error::rejected("bad_phase", format!("unknown phase {other}"))),
        })
    }

    pub fn is_closed(self) -> bool {
        matches!(self, Phase::Done | Phase::Abandoned)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub body: String,
    pub executor: String,
    pub phase: Phase,
    pub paused: bool,
    pub blocked_reason: Option<String>,
    pub criteria_version: i64,
    pub queue_pos: Option<i64>,
    pub revision: i64,
    pub created_at: i64,
    pub closed_at: Option<i64>,
}

const TASK_COLUMNS: &str = "id, title, body, executor, phase, paused, blocked_reason, criteria_version, \
                            queue_pos, revision, created_at, closed_at";

fn task_from_row(r: &Row<'_>) -> rusqlite::Result<(Task, String)> {
    let phase: String = r.get(4)?;
    Ok((
        Task {
            id: r.get(0)?,
            title: r.get(1)?,
            body: r.get(2)?,
            executor: r.get(3)?,
            phase: Phase::Queued, // replaced by the caller after parsing `phase`
            paused: r.get::<_, i64>(5)? != 0,
            blocked_reason: r.get(6)?,
            criteria_version: r.get(7)?,
            queue_pos: r.get(8)?,
            revision: r.get(9)?,
            created_at: r.get(10)?,
            closed_at: r.get(11)?,
        },
        phase,
    ))
}

pub fn load_task(conn: &Connection, id: &str) -> Result<Task> {
    let row = conn
        .query_row(&format!("SELECT {TASK_COLUMNS} FROM task WHERE id = ?1"), [id], task_from_row)
        .optional()?;
    let (mut task, phase) = row.ok_or_else(|| Error::rejected("not_found", format!("no task {id}")))?;
    task.phase = Phase::parse(&phase)?;
    Ok(task)
}

pub fn list_tasks(conn: &Connection) -> Result<Vec<Task>> {
    let mut stmt = conn.prepare(&format!("SELECT {TASK_COLUMNS} FROM task ORDER BY created_at, id"))?;
    let rows = stmt.query_map([], task_from_row)?;
    rows.map(|r| {
        let (mut task, phase) = r?;
        task.phase = Phase::parse(&phase)?;
        Ok(task)
    })
    .collect()
}

/// The task currently occupying a role, if any.
pub fn occupant(conn: &Connection, role: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT task_id FROM occupancy WHERE role = ?1", [role], |r| r.get(0))
        .optional()?)
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Message {
    pub id: String,
    pub seq: i64,
    pub role: String,
    pub task_id: Option<String>,
    pub body: String,
}

/// Messages waiting in a role's inbox, in arrival order (data-model.md §3.4).
pub fn queued_messages(conn: &Connection, role: &str) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(
        "SELECT id, seq, role, task_id, body FROM message WHERE role = ?1 AND state = 'queued' ORDER BY seq",
    )?;
    let rows = stmt.query_map([role], |r| {
        Ok(Message { id: r.get(0)?, seq: r.get(1)?, role: r.get(2)?, task_id: r.get(3)?, body: r.get(4)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn require_role(cx: &Cx<'_>, role: &str) -> Result<()> {
    let exists = cx.tx.query_row("SELECT 1 FROM role WHERE name = ?1", [role], |_| Ok(())).optional()?;
    exists.ok_or_else(|| Error::rejected("unknown_role", format!("no role {role}")))
}

fn require_open(task: &Task) -> Result<()> {
    if task.phase.is_closed() {
        return Err(Error::rejected("task_closed", format!("task {} is {}", task.id, task.phase.as_str())));
    }
    Ok(())
}

fn bump_revision(cx: &Cx<'_>, task_id: &str) -> Result<()> {
    cx.tx.execute("UPDATE task SET revision = revision + 1 WHERE id = ?1", [task_id])?;
    Ok(())
}

fn next_queue_pos(cx: &Cx<'_>, executor: &str) -> Result<i64> {
    Ok(cx.tx.query_row(
        "SELECT COALESCE(MAX(queue_pos), 0) + 1 FROM task WHERE executor = ?1 AND phase = 'queued'",
        [executor],
        |r| r.get(0),
    )?)
}

fn record_decision(cx: &Cx<'_>, kind: &str, task_id: &str, caller: &Caller, detail: serde_json::Value) -> Result<String> {
    let id = new_id("dec");
    cx.tx.execute(
        "INSERT INTO decision (id, kind, task_id, actor, detail, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, kind, task_id, caller.scope(), detail.to_string(), cx.now],
    )?;
    Ok(id)
}

/// Puts a message in a role's inbox. Delivery is the scheduler's job (data-model.md §3.4, §4.2).
pub fn queue_message(cx: &mut Cx<'_>, role: &str, source: &Caller, task_id: Option<&str>, body: &str) -> Result<String> {
    let id = new_id("msg");
    let seq: i64 = cx.tx.query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM message", [], |r| r.get(0))?;
    cx.tx.execute(
        "INSERT INTO message (id, seq, role, source, task_id, body, state, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7)",
        params![id, seq, role, source.scope(), task_id, body, cx.now],
    )?;
    cx.emit("message.queued", &id, json!({ "role": role, "task_id": task_id }))?;
    Ok(id)
}

// ---- user commands ----

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTask {
    pub request_id: String,
    pub title: String,
    pub body: String,
    pub criteria: String,
    pub executor: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Created {
    pub id: String,
}

impl Command for CreateTask {
    const NAME: &'static str = "create_task";
    type Output = Created;

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<Created> {
        caller.require_user()?;
        require_role(cx, &self.executor)?;
        if self.title.trim().is_empty() {
            return Err(Error::rejected("empty_title", "a task needs a title"));
        }
        let id = new_id("task");
        let pos = next_queue_pos(cx, &self.executor)?;
        cx.tx.execute(
            "INSERT INTO task (id, title, body, executor, phase, criteria_version, queue_pos, revision, created_at)
             VALUES (?1, ?2, ?3, ?4, 'queued', 1, ?5, 1, ?6)",
            params![id, self.title, self.body, self.executor, pos, cx.now],
        )?;
        cx.tx.execute(
            "INSERT INTO criteria_version (task_id, version, text, created_by, created_at) VALUES (?1, 1, ?2, ?3, ?4)",
            params![id, self.criteria, caller.scope(), cx.now],
        )?;
        cx.emit("task.created", &id, json!({ "executor": self.executor }))?;
        Ok(Created { id })
    }
}

/// Edits the completion criteria. Each edit is a new version; existing evidence stays bound to
/// the old one (data-model.md §4.4).
#[derive(Debug, Serialize, Deserialize)]
pub struct EditCriteria {
    pub request_id: String,
    pub task_id: String,
    pub expected_version: i64,
    pub text: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct CriteriaEdited {
    pub version: i64,
}

impl Command for EditCriteria {
    const NAME: &'static str = "edit_criteria";
    type Output = CriteriaEdited;

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<CriteriaEdited> {
        caller.require_user()?;
        let task = load_task(cx.tx, &self.task_id)?;
        require_open(&task)?;
        if task.criteria_version != self.expected_version {
            return Err(Error::rejected(
                "stale_version",
                format!("criteria are at version {}, not {}", task.criteria_version, self.expected_version),
            ));
        }
        let version = task.criteria_version + 1;
        cx.tx.execute(
            "INSERT INTO criteria_version (task_id, version, text, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task.id, version, self.text, caller.scope(), cx.now],
        )?;
        cx.tx.execute("UPDATE task SET criteria_version = ?2 WHERE id = ?1", params![task.id, version])?;
        bump_revision(cx, &task.id)?;
        // Once work has started the executor must hear about it. The scheduler holds the message
        // back while the task is outside execution (data-model.md §4.2).
        if task.phase != Phase::Queued {
            let body = format!("完成条件已更新为第 {version} 版：\n{}", self.text);
            queue_message(cx, &task.executor, &Caller::Runtime, Some(&task.id), &body)?;
        }
        cx.emit("task.criteria_changed", &task.id, json!({ "version": version }))?;
        Ok(CriteriaEdited { version })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendMessage {
    pub request_id: String,
    pub role: String,
    pub task_id: Option<String>,
    pub body: String,
}

impl Command for SendMessage {
    const NAME: &'static str = "send_message";
    type Output = Created;

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<Created> {
        caller.require_user()?;
        require_role(cx, &self.role)?;
        if let Some(task_id) = &self.task_id {
            load_task(cx.tx, task_id)?;
        }
        let id = queue_message(cx, &self.role, caller, self.task_id.as_deref(), &self.body)?;
        Ok(Created { id })
    }
}

/// User pause. Kept separate from phase and blocking reason; quota recovery never clears it
/// (roles-and-tasks.md §2.2).
#[derive(Debug, Serialize, Deserialize)]
pub struct SetPaused {
    pub request_id: String,
    pub task_id: String,
    pub paused: bool,
}

impl Command for SetPaused {
    const NAME: &'static str = "set_paused";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user()?;
        let task = load_task(cx.tx, &self.task_id)?;
        require_open(&task)?;
        cx.tx.execute("UPDATE task SET paused = ?2 WHERE id = ?1", params![task.id, self.paused])?;
        bump_revision(cx, &task.id)?;
        cx.emit("task.paused_changed", &task.id, json!({ "paused": self.paused }))?;
        Ok(())
    }
}

/// Moves a queued task to a new position in its executor's queue.
#[derive(Debug, Serialize, Deserialize)]
pub struct MoveInQueue {
    pub request_id: String,
    pub task_id: String,
    pub to_index: usize,
}

impl Command for MoveInQueue {
    const NAME: &'static str = "move_in_queue";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user()?;
        let task = load_task(cx.tx, &self.task_id)?;
        if task.phase != Phase::Queued {
            return Err(Error::rejected("not_queued", format!("task {} is {}", task.id, task.phase.as_str())));
        }
        let mut order: Vec<String> = {
            let mut stmt = cx
                .tx
                .prepare("SELECT id FROM task WHERE executor = ?1 AND phase = 'queued' ORDER BY queue_pos, id")?;
            stmt.query_map([&task.executor], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        order.retain(|id| id != &task.id);
        order.insert(self.to_index.min(order.len()), task.id.clone());
        for (i, id) in order.iter().enumerate() {
            cx.tx.execute("UPDATE task SET queue_pos = ?2 WHERE id = ?1", params![id, i as i64 + 1])?;
        }
        cx.emit("task.queue_changed", &task.executor, json!({ "order": order }))?;
        Ok(())
    }
}

/// Stops all further work on the task and keeps its evidence. Unaccepted work never reached the
/// integration version, so nothing needs reverting (roles-and-tasks.md §2.2).
#[derive(Debug, Serialize, Deserialize)]
pub struct Abandon {
    pub request_id: String,
    pub task_id: String,
    pub reason: String,
}

impl Command for Abandon {
    const NAME: &'static str = "abandon";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user()?;
        let task = load_task(cx.tx, &self.task_id)?;
        require_open(&task)?;
        record_decision(cx, "abandon", &task.id, caller, json!({ "reason": self.reason }))?;
        cx.tx.execute(
            "UPDATE attempt SET ended_at = ?2, end_reason = 'abandoned' WHERE task_id = ?1 AND ended_at IS NULL",
            params![task.id, cx.now],
        )?;
        cx.tx.execute("DELETE FROM occupancy WHERE task_id = ?1", [&task.id])?;
        cx.tx.execute(
            "UPDATE task SET phase = 'abandoned', closed_at = ?2, queue_pos = NULL WHERE id = ?1",
            params![task.id, cx.now],
        )?;
        bump_revision(cx, &task.id)?;
        cx.emit("task.abandoned", &task.id, json!({ "reason": self.reason }))?;
        Ok(())
    }
}

/// Reopens a closed task. The next attempt starts from the latest candidate rebased onto the
/// integration version (data-model.md §4.6); that is resolved when the attempt starts.
#[derive(Debug, Serialize, Deserialize)]
pub struct Reopen {
    pub request_id: String,
    pub task_id: String,
}

impl Command for Reopen {
    const NAME: &'static str = "reopen";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user()?;
        let task = load_task(cx.tx, &self.task_id)?;
        if !task.phase.is_closed() {
            return Err(Error::rejected("not_closed", format!("task {} is {}", task.id, task.phase.as_str())));
        }
        record_decision(cx, "reopen", &task.id, caller, json!({}))?;
        let pos = next_queue_pos(cx, &task.executor)?;
        cx.tx.execute(
            "UPDATE task SET phase = 'queued', closed_at = NULL, queue_pos = ?2 WHERE id = ?1",
            params![task.id, pos],
        )?;
        bump_revision(cx, &task.id)?;
        cx.emit("task.reopened", &task.id, json!({}))?;
        Ok(())
    }
}

// ---- runtime commands ----

/// Starts a new attempt: occupies the executor and moves the task into execution
/// (data-model.md §2, §4.1). Quota and materialization preconditions arrive with later increments.
#[derive(Debug, Serialize, Deserialize)]
pub struct StartAttempt {
    pub task_id: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct AttemptStarted {
    pub attempt_id: String,
    pub seq: i64,
}

impl Command for StartAttempt {
    const NAME: &'static str = "start_attempt";
    type Output = AttemptStarted;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<AttemptStarted> {
        caller.require_runtime()?;
        let task = load_task(cx.tx, &self.task_id)?;
        if task.phase != Phase::Queued {
            return Err(Error::rejected("not_queued", format!("task {} is {}", task.id, task.phase.as_str())));
        }
        if task.paused {
            return Err(Error::rejected("paused", format!("task {} is paused", task.id)));
        }
        if let Some(other) = occupant(cx.tx, &task.executor)? {
            return Err(Error::rejected("role_busy", format!("{} is occupied by {other}", task.executor)));
        }
        let seq: i64 = cx.tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM attempt WHERE task_id = ?1",
            [&task.id],
            |r| r.get(0),
        )?;
        let attempt_id = new_id("att");
        cx.tx.execute(
            "INSERT INTO attempt (id, task_id, seq, started_at) VALUES (?1, ?2, ?3, ?4)",
            params![attempt_id, task.id, seq, cx.now],
        )?;
        cx.tx.execute(
            "INSERT INTO occupancy (role, task_id, since) VALUES (?1, ?2, ?3)",
            params![task.executor, task.id, cx.now],
        )?;
        cx.tx.execute("UPDATE task SET phase = 'executing', queue_pos = NULL WHERE id = ?1", [&task.id])?;
        bump_revision(cx, &task.id)?;
        cx.emit("attempt.started", &task.id, json!({ "attempt_id": attempt_id, "seq": seq }))?;
        Ok(AttemptStarted { attempt_id, seq })
    }
}
