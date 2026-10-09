//! Tasks, completion criteria, attempts, task-level occupancy and queued messages
//! (data-model.md §2, §3.4, §4; roles-and-tasks.md §2).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::id::new_id;
use crate::prompts;
use crate::sql::sql_enum;

sql_enum! {
    pub enum Phase {
        Queued = "queued",
        Executing = "executing",
        Verifying = "verifying",
        Accepting = "accepting",
        Done = "done",
        Abandoned = "abandoned",
    }
}

impl Phase {
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
    /// For a task made from changes outside any task: their capture, where its work starts (#14).
    pub origin_capture: Option<String>,
}

const TASK_COLUMNS: &str = "id, title, body, executor, phase, paused, blocked_reason, criteria_version, \
                            queue_pos, revision, created_at, closed_at, origin_capture";

fn task_from_row(r: &Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        title: r.get(1)?,
        body: r.get(2)?,
        executor: r.get(3)?,
        phase: r.get(4)?,
        paused: r.get::<_, i64>(5)? != 0,
        blocked_reason: r.get(6)?,
        criteria_version: r.get(7)?,
        queue_pos: r.get(8)?,
        revision: r.get(9)?,
        created_at: r.get(10)?,
        closed_at: r.get(11)?,
        origin_capture: r.get(12)?,
    })
}

pub fn load_task(conn: &Connection, id: &str) -> Result<Task> {
    let row =
        conn.query_row(&format!("SELECT {TASK_COLUMNS} FROM task WHERE id = ?1"), [id], task_from_row).optional()?;
    row.ok_or_else(|| Error::rejected("not_found", format!("no task {id}")))
}

pub fn list_tasks(conn: &Connection) -> Result<Vec<Task>> {
    let mut stmt = conn.prepare(&format!("SELECT {TASK_COLUMNS} FROM task ORDER BY created_at, id"))?;
    let rows = stmt.query_map([], task_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The first queued, unpaused task of the role: the next one to start (roles-and-tasks.md §2.2).
pub fn next_queued_task(conn: &Connection, role: &str) -> Result<Option<Task>> {
    let row = conn
        .query_row(
            &format!(
                "SELECT {TASK_COLUMNS} FROM task WHERE executor = ?1 AND phase = 'queued' AND paused = 0
                 ORDER BY queue_pos, id LIMIT 1"
            ),
            [role],
            task_from_row,
        )
        .optional()?;
    Ok(row)
}

/// The task currently occupying a role, if any.
pub fn occupant(conn: &Connection, role: &str) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT task_id FROM occupancy WHERE role = ?1", [role], |r| r.get(0)).optional()?)
}

sql_enum! {
    /// How an attempt ended.
    pub enum AttemptEnd {
        /// Its executor's done was captured as the candidate.
        Candidate = "candidate",
        Abandoned = "abandoned",
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Attempt {
    pub id: String,
    pub task_id: String,
    pub seq: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub end_reason: Option<AttemptEnd>,
    /// Where its code starts; `None` when the slot stayed as it was (data-model.md §4.3).
    pub code_start: Option<String>,
    pub done_turn_id: Option<String>,
    pub candidate_id: Option<String>,
    /// Files its code start left with conflict markers.
    pub conflicts: Vec<String>,
}

fn attempts(conn: &Connection, filter: &str, task_id: &str) -> Result<Vec<Attempt>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, task_id, seq, started_at, ended_at, end_reason, code_start, done_turn_id, candidate_id, conflicts
         FROM attempt WHERE task_id = ?1 {filter}"
    ))?;
    let rows = stmt.query_map([task_id], |r| {
        let attempt = Attempt {
            id: r.get(0)?,
            task_id: r.get(1)?,
            seq: r.get(2)?,
            started_at: r.get(3)?,
            ended_at: r.get(4)?,
            end_reason: r.get(5)?,
            code_start: r.get(6)?,
            done_turn_id: r.get(7)?,
            candidate_id: r.get(8)?,
            conflicts: vec![],
        };
        Ok((attempt, r.get::<_, Option<String>>(9)?))
    })?;
    rows.map(|row| {
        let (mut attempt, conflicts) = row?;
        attempt.conflicts = conflicts.as_deref().map(serde_json::from_str).transpose()?.unwrap_or_default();
        Ok(attempt)
    })
    .collect()
}

/// The task's open attempt, if any (data-model.md §4.1).
pub fn open_attempt(conn: &Connection, task_id: &str) -> Result<Option<Attempt>> {
    Ok(attempts(conn, "AND ended_at IS NULL", task_id)?.pop())
}

/// The task's attempts, the first first.
pub fn list_attempts(conn: &Connection, task_id: &str) -> Result<Vec<Attempt>> {
    attempts(conn, "ORDER BY seq", task_id)
}

/// The task's latest attempt, open or ended.
pub fn latest_attempt(conn: &Connection, task_id: &str) -> Result<Option<Attempt>> {
    Ok(attempts(conn, "ORDER BY seq DESC LIMIT 1", task_id)?.pop())
}

pub fn criteria_text(conn: &Connection, task_id: &str, version: i64) -> Result<String> {
    Ok(conn.query_row(
        "SELECT text FROM criteria_version WHERE task_id = ?1 AND version = ?2",
        params![task_id, version],
        |r| r.get(0),
    )?)
}

/// The task as the executor first sees it: the user's words and the current criteria. Sent when
/// an attempt starts and when the role starts a new native session mid-attempt. The first line
/// doubles as its summary in the GUI; the task id is in the message header (`compose_input`).
pub fn brief(conn: &Connection, task: &Task, attempt_seq: i64) -> Result<String> {
    let criteria = criteria_text(conn, &task.id, task.criteria_version)?;
    Ok(prompts::brief(&task.title, &task.body, attempt_seq, task.criteria_version, &criteria))
}

/// What the executor is told when an attempt starts, and again when a new native session takes
/// the attempt over (data-model.md §4.1): the brief, then what is special about where its code
/// starts. A new session used to get the brief alone and never heard of conflict markers in its
/// slot (#16).
pub fn attempt_brief(conn: &Connection, task: &Task, seq: i64, conflicts: &[String]) -> Result<String> {
    let mut text = brief(conn, task, seq)?;
    if task.origin_capture.is_some() && seq == 1 {
        text.push_str("\n\n");
        text.push_str(prompts::STARTS_FROM_OUTSIDE_CHANGES);
    }
    if !conflicts.is_empty() {
        text.push_str("\n\n");
        text.push_str(&prompts::conflicts_at_start(conflicts));
    }
    Ok(text)
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Message {
    pub id: String,
    pub seq: i64,
    pub role: String,
    /// The caller scope that sent it: `user`, `runtime` or `role:<name>`.
    pub source: String,
    pub task_id: Option<String>,
    pub body: String,
    /// The turn it was delivered in, once bound.
    pub turn_id: Option<String>,
    pub created_at: i64,
}

/// Messages waiting in a role's inbox, in arrival order (data-model.md §3.4).
pub fn queued_messages(conn: &Connection, role: &str) -> Result<Vec<Message>> {
    messages(conn, "role = ?1 AND state = 'queued'", role)
}

/// The messages a turn delivered, in arrival order.
pub fn turn_messages(conn: &Connection, turn_id: &str) -> Result<Vec<Message>> {
    messages(conn, "turn_id = ?1", turn_id)
}

/// The user's messages to a task that its executor has not got: sent after the executor reported
/// done, they wait while the task is out of execution (data-model.md §4.2). Accepting the task
/// needs the user to let them go (#14).
pub fn undelivered_user_messages(conn: &Connection, task_id: &str) -> Result<Vec<Message>> {
    messages(conn, "task_id = ?1 AND state = 'queued' AND source = 'user'", task_id)
}

/// A closed task's queued messages are not delivered: its executor no longer works on it
/// (data-model.md §4.2, #14). The runtime's own, such as a criteria update, lose their point too.
pub(crate) fn drop_queued(cx: &mut Cx<'_>, task_id: &str) -> Result<()> {
    let dropped = messages(cx.tx, "task_id = ?1 AND state = 'queued'", task_id)?;
    if dropped.is_empty() {
        return Ok(());
    }
    cx.tx.execute("UPDATE message SET state = 'dropped' WHERE task_id = ?1 AND state = 'queued'", [task_id])?;
    let ids: Vec<&str> = dropped.iter().map(|m| m.id.as_str()).collect();
    cx.emit("message.dropped", task_id, json!({ "messages": ids }))?;
    Ok(())
}

/// The user's messages a decision let go, as the decision records them.
pub(crate) fn dropped_detail(messages: &[Message]) -> serde_json::Value {
    json!(messages.iter().map(|m| json!({ "id": m.id, "body": m.body })).collect::<Vec<_>>())
}

fn messages(conn: &Connection, filter: &str, arg: &str) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, seq, role, source, task_id, body, turn_id, created_at FROM message WHERE {filter} ORDER BY seq"
    ))?;
    let rows = stmt.query_map([arg], |r| {
        Ok(Message {
            id: r.get(0)?,
            seq: r.get(1)?,
            role: r.get(2)?,
            source: r.get(3)?,
            task_id: r.get(4)?,
            body: r.get(5)?,
            turn_id: r.get(6)?,
            created_at: r.get(7)?,
        })
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

/// Puts the task into execution with a new attempt (data-model.md §4.1). Every way into
/// execution goes through here: a start, a reopen, a failed verification, a send-back. A
/// question the executor asked in an earlier attempt no longer stands (#16). The caller emits
/// `attempt.started` once the attempt's slot and message are in place.
pub(crate) fn enter_execution(
    cx: &mut Cx<'_>,
    task: &Task,
    code_start: Option<&str>,
    conflicts: &[String],
) -> Result<AttemptStarted> {
    let seq: i64 =
        cx.tx
            .query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM attempt WHERE task_id = ?1", [&task.id], |r| r.get(0))?;
    let attempt_id = new_id("att");
    let conflicts = (!conflicts.is_empty()).then(|| serde_json::to_string(conflicts)).transpose()?;
    cx.tx.execute(
        "INSERT INTO attempt (id, task_id, seq, started_at, code_start, conflicts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![attempt_id, task.id, seq, cx.now, code_start, conflicts],
    )?;
    cx.tx.execute(
        "UPDATE task SET phase = 'executing', blocked_reason = NULL, queue_pos = NULL WHERE id = ?1",
        [&task.id],
    )?;
    bump_revision(cx, &task.id)?;
    Ok(AttemptStarted { attempt_id, seq })
}

/// Closes the task, accepted (`Done`) or abandoned (data-model.md §4.2, §9.2). Every way out goes
/// through here: the executor is freed, the task's sessions end (#11) and the messages still
/// queued for it are dropped (#14). An abandoned task's open attempt ends with it; an accepted
/// task's attempt ended when its candidate was captured.
pub(crate) fn close_task(cx: &mut Cx<'_>, task: &Task, phase: Phase) -> Result<()> {
    debug_assert!(phase.is_closed(), "close_task with {}", phase.as_str());
    if phase == Phase::Abandoned {
        cx.tx.execute(
            "UPDATE attempt SET ended_at = ?2, end_reason = 'abandoned' WHERE task_id = ?1 AND ended_at IS NULL",
            params![task.id, cx.now],
        )?;
    }
    cx.tx.execute("DELETE FROM occupancy WHERE task_id = ?1", [&task.id])?;
    crate::turn::end_task_sessions(cx, &task.id)?;
    drop_queued(cx, &task.id)?;
    cx.tx.execute(
        "UPDATE task SET phase = ?2, closed_at = ?3, queue_pos = NULL, blocked_reason = NULL WHERE id = ?1",
        params![task.id, phase.as_str(), cx.now],
    )?;
    bump_revision(cx, &task.id)
}

fn next_queue_pos(cx: &Cx<'_>, executor: &str) -> Result<i64> {
    Ok(cx.tx.query_row(
        "SELECT COALESCE(MAX(queue_pos), 0) + 1 FROM task WHERE executor = ?1 AND phase = 'queued'",
        [executor],
        |r| r.get(0),
    )?)
}

pub(crate) fn record_decision(
    cx: &Cx<'_>,
    kind: &str,
    task_id: Option<&str>,
    caller: &Caller,
    detail: serde_json::Value,
) -> Result<String> {
    let id = new_id("dec");
    cx.tx.execute(
        "INSERT INTO decision (id, kind, task_id, actor, detail, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, kind, task_id, caller.scope(), detail.to_string(), cx.now],
    )?;
    Ok(id)
}

/// Puts a message in a role's inbox. Delivery is the scheduler's job (data-model.md §3.4, §4.2).
pub fn queue_message(
    cx: &mut Cx<'_>,
    role: &str,
    source: &Caller,
    task_id: Option<&str>,
    body: &str,
) -> Result<String> {
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
        let id = create_task(cx, caller, &self.title, &self.body, &self.criteria, &self.executor)?;
        Ok(Created { id })
    }
}

fn create_task(
    cx: &mut Cx<'_>,
    caller: &Caller,
    title: &str,
    body: &str,
    criteria: &str,
    executor: &str,
) -> Result<String> {
    require_role(cx, executor)?;
    if title.trim().is_empty() {
        return Err(Error::rejected("empty_title", "a task needs a title"));
    }
    let id = new_id("task");
    let pos = next_queue_pos(cx, executor)?;
    cx.tx.execute(
        "INSERT INTO task (id, title, body, executor, phase, criteria_version, queue_pos, revision, created_at)
         VALUES (?1, ?2, ?3, ?4, 'queued', 1, ?5, 1, ?6)",
        params![id, title, body, executor, pos, cx.now],
    )?;
    cx.tx.execute(
        "INSERT INTO criteria_version (task_id, version, text, created_by, created_at) VALUES (?1, 1, ?2, ?3, ?4)",
        params![id, criteria, caller.scope(), cx.now],
    )?;
    cx.emit("task.created", &id, json!({ "executor": executor }))?;
    Ok(id)
}

/// "建成任务": a task made from the changes a turn outside any task left in its executor's slot.
/// Its first attempt starts from them, as a reopened task starts from its last candidate
/// (data-model.md §4.6, #14).
#[derive(Debug, Serialize, Deserialize)]
pub struct AdoptOutsideChanges {
    pub request_id: String,
    pub capture_id: String,
    pub title: String,
    pub body: String,
    pub criteria: String,
}

impl Command for AdoptOutsideChanges {
    const NAME: &'static str = "adopt_outside_changes";
    type Output = Created;

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<Created> {
        caller.require_user()?;
        let capture = crate::capture::settle_outside(cx, &self.capture_id, crate::capture::Outside::Adopted)?;
        // The changes are in this role's slot; its executor carries on with them.
        let id = create_task(cx, caller, &self.title, &self.body, &self.criteria, &capture.role)?;
        cx.tx.execute("UPDATE task SET origin_capture = ?2 WHERE id = ?1", params![id, capture.id])?;
        record_decision(cx, "adopt_outside_changes", Some(&id), caller, json!({ "capture_id": capture.id }))?;
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
            let body = prompts::criteria_updated(version, &self.text);
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
///
/// The queue keeps its existing position values and hands them out in the new order, so only
/// tasks whose position really changes are written, and each of them gets a new revision
/// (data-model.md §9.1). Moving a task to where it already is changes nothing and emits no event.
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
        let queue: Vec<(String, i64)> = {
            let mut stmt = cx.tx.prepare(
                "SELECT id, queue_pos FROM task WHERE executor = ?1 AND phase = 'queued' ORDER BY queue_pos, id",
            )?;
            stmt.query_map([&task.executor], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
        };
        let mut order: Vec<&str> = queue.iter().map(|(id, _)| id.as_str()).filter(|id| *id != task.id).collect();
        order.insert(self.to_index.min(order.len()), &task.id);

        let mut changed = false;
        for (id, (_, pos)) in order.iter().zip(&queue) {
            let old = queue.iter().find(|(other, _)| other == id).map(|(_, p)| *p);
            if old != Some(*pos) {
                cx.tx.execute("UPDATE task SET queue_pos = ?2 WHERE id = ?1", params![id, pos])?;
                bump_revision(cx, id)?;
                changed = true;
            }
        }
        if changed {
            cx.emit("task.queue_changed", &task.executor, json!({ "order": order }))?;
        }
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
        let dropped = undelivered_user_messages(cx.tx, &task.id)?;
        let detail = json!({ "reason": self.reason, "dropped_messages": dropped_detail(&dropped) });
        record_decision(cx, "abandon", Some(&task.id), caller, detail)?;
        close_task(cx, &task, Phase::Abandoned)?;
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
        record_decision(cx, "reopen", Some(&task.id), caller, json!({}))?;
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

/// Starts a new attempt: occupies the executor, moves the task into execution, plans the slot at
/// the attempt's code start and puts the brief in the executor's inbox (data-model.md §2, §4.1,
/// §4.6; roles-and-tasks.md §2.2). The slot goes to this work only once its current content is
/// captured (harness-adapter.md §3).
#[derive(Debug, Serialize, Deserialize)]
pub struct StartAttempt {
    pub task_id: String,
    /// For a reopened task with an earlier candidate: that candidate rebased onto the
    /// integration version, which the runtime prepares in the store first (data-model.md §4.6).
    /// Every other attempt starts from the integration version.
    pub code_start: Option<CodeStart>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeStart {
    pub commit: String,
    /// The integration version it was rebased onto.
    pub base: String,
    pub conflicts: Vec<String>,
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
        // The slot is about to be rewritten.
        crate::workspace::require_slot_free(cx.tx, &task.executor, None)?;
        let project = crate::project::require_project(cx.tx)?;
        let earlier = crate::verify::work_so_far(cx.tx, &task.id)?;
        let start = match (&earlier, &self.code_start) {
            (Some(_), Some(start)) if start.base == project.integration => start.clone(),
            (Some(_), _) => {
                return Err(Error::rejected(
                    "code_start_needed",
                    "rebase the task's work so far onto the integration version",
                ));
            }
            (None, _) => {
                CodeStart { commit: project.integration.clone(), base: project.integration.clone(), conflicts: vec![] }
            }
        };

        let started = enter_execution(cx, &task, Some(&start.commit), &start.conflicts)?;
        let seq = started.seq;
        cx.tx.execute(
            "INSERT INTO occupancy (role, task_id, since) VALUES (?1, ?2, ?3)",
            params![task.executor, task.id, cx.now],
        )?;
        if let Some(slot) = crate::workspace::role_slot(cx.tx, &task.executor)? {
            crate::workspace::plan(cx, &slot, &start.commit, &start.base, Some(&task.id))?;
        }
        let brief = attempt_brief(cx.tx, &task, seq, &start.conflicts)?;
        queue_message(cx, &task.executor, &Caller::Runtime, Some(&task.id), &brief)?;
        cx.emit("attempt.started", &task.id, json!({ "attempt_id": started.attempt_id, "seq": seq }))?;
        Ok(started)
    }
}
