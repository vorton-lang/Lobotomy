//! Native sessions, turns and message delivery (data-model.md §2, §3).
//!
//! A turn is one CLI run. The runtime registers it before starting the CLI, binding every queued
//! message of the role in the same transaction; the turn's MCP token identifies the role's calls.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::id::new_id;
use crate::task::{self, Message, Phase, brief, load_task, occupant, open_attempt, queue_message, queued_messages};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    /// The turn record exists; the CLI has not started.
    Registered,
    Running,
    Ended,
    /// The backend restarted while the turn was registered or running (data-model.md §3.3).
    Unknown,
}

impl TurnState {
    pub fn as_str(self) -> &'static str {
        match self {
            TurnState::Registered => "registered",
            TurnState::Running => "running",
            TurnState::Ended => "ended",
            TurnState::Unknown => "unknown",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "registered" => TurnState::Registered,
            "running" => TurnState::Running,
            "ended" => TurnState::Ended,
            "unknown" => TurnState::Unknown,
            other => return Err(Error::rejected("bad_state", format!("unknown turn state {other}"))),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    Failed,
    Interrupted,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Completed => "completed",
            Outcome::Failed => "failed",
            Outcome::Interrupted => "interrupted",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "completed" => Outcome::Completed,
            "failed" => Outcome::Failed,
            "interrupted" => Outcome::Interrupted,
            other => return Err(Error::rejected("bad_outcome", format!("unknown outcome {other}"))),
        })
    }
}

/// Why a turn failed (data-model.md §3.2, §8.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
    /// When the quota resets, in Unix milliseconds, if the harness said so.
    pub resets_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Quota,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Turn {
    pub id: String,
    pub role: String,
    pub harness: String,
    pub native_session_id: String,
    /// The harness's own session id, known after the session's first turn started.
    pub native_id: Option<String>,
    pub task_id: Option<String>,
    pub attempt_id: Option<String>,
    pub token: String,
    pub input: String,
    pub state: TurnState,
    pub outcome: Option<Outcome>,
    pub failure: Option<Failure>,
    pub pid: Option<i64>,
    pub process_start: Option<i64>,
    pub done_at: Option<i64>,
    pub registered_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

const TURN_SELECT: &str = "SELECT t.id, t.role, s.harness, t.native_session_id, s.native_id, t.task_id, t.attempt_id,
       t.token, t.input, t.state, t.outcome, t.failure, t.pid, t.process_start, t.done_at,
       t.registered_at, t.started_at, t.ended_at
     FROM turn t JOIN native_session s ON s.id = t.native_session_id";

struct TurnRow {
    turn: Turn,
    state: String,
    outcome: Option<String>,
    failure: Option<String>,
}

fn turn_from_row(r: &Row<'_>) -> rusqlite::Result<TurnRow> {
    Ok(TurnRow {
        turn: Turn {
            id: r.get(0)?,
            role: r.get(1)?,
            harness: r.get(2)?,
            native_session_id: r.get(3)?,
            native_id: r.get(4)?,
            task_id: r.get(5)?,
            attempt_id: r.get(6)?,
            token: r.get(7)?,
            input: r.get(8)?,
            state: TurnState::Registered, // replaced after parsing
            outcome: None,
            failure: None,
            pid: r.get(12)?,
            process_start: r.get(13)?,
            done_at: r.get(14)?,
            registered_at: r.get(15)?,
            started_at: r.get(16)?,
            ended_at: r.get(17)?,
        },
        state: r.get(9)?,
        outcome: r.get(10)?,
        failure: r.get(11)?,
    })
}

fn finish(row: TurnRow) -> Result<Turn> {
    let mut turn = row.turn;
    turn.state = TurnState::parse(&row.state)?;
    turn.outcome = row.outcome.as_deref().map(Outcome::parse).transpose()?;
    turn.failure = row.failure.as_deref().map(serde_json::from_str).transpose()?;
    Ok(turn)
}

fn query_turns(conn: &Connection, filter: &str, args: impl rusqlite::Params) -> Result<Vec<Turn>> {
    let mut stmt = conn.prepare(&format!("{TURN_SELECT} {filter}"))?;
    let rows = stmt.query_map(args, turn_from_row)?;
    rows.map(|r| finish(r?)).collect()
}

pub fn load_turn(conn: &Connection, id: &str) -> Result<Turn> {
    query_turns(conn, "WHERE t.id = ?1", [id])?
        .pop()
        .ok_or_else(|| Error::rejected("not_found", format!("no turn {id}")))
}

/// The turn an MCP token was issued to (data-model.md §3.1).
pub fn turn_by_token(conn: &Connection, token: &str) -> Result<Option<Turn>> {
    Ok(query_turns(conn, "WHERE t.token = ?1", [token])?.pop())
}

/// Turns in the given state, oldest first.
pub fn turns_in_state(conn: &Connection, state: TurnState) -> Result<Vec<Turn>> {
    query_turns(conn, "WHERE t.state = ?1 ORDER BY t.registered_at, t.id", [state.as_str()])
}

/// The latest turn of the role's current native session.
pub fn last_turn(conn: &Connection, role: &str) -> Result<Option<Turn>> {
    Ok(query_turns(
        conn,
        "WHERE s.role = ?1 AND s.ended_at IS NULL ORDER BY t.registered_at DESC, t.id DESC LIMIT 1",
        [role],
    )?
    .pop())
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NativeSession {
    pub id: String,
    pub role: String,
    pub harness: String,
    pub native_id: Option<String>,
}

pub fn current_session(conn: &Connection, role: &str) -> Result<Option<NativeSession>> {
    Ok(conn
        .query_row(
            "SELECT id, role, harness, native_id FROM native_session WHERE role = ?1 AND ended_at IS NULL",
            [role],
            |r| Ok(NativeSession { id: r.get(0)?, role: r.get(1)?, harness: r.get(2)?, native_id: r.get(3)? }),
        )
        .optional()?)
}

fn role_harness(conn: &Connection, role: &str) -> Result<String> {
    conn.query_row("SELECT harness FROM role WHERE name = ?1", [role], |r| r.get(0))
        .optional()?
        .ok_or_else(|| Error::rejected("unknown_role", format!("no role {role}")))
}

fn start_session(cx: &mut Cx<'_>, role: &str) -> Result<NativeSession> {
    let harness = role_harness(cx.tx, role)?;
    let id = new_id("ns");
    cx.tx.execute(
        "INSERT INTO native_session (id, role, harness, started_at) VALUES (?1, ?2, ?3, ?4)",
        params![id, role, harness, cx.now],
    )?;
    cx.emit("native_session.started", &id, json!({ "role": role }))?;
    Ok(NativeSession { id, role: role.to_owned(), harness, native_id: None })
}

/// The text written to the CLI's stdin: the bound messages in arrival order.
pub fn compose_input(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|m| {
            let from = match m.source.as_str() {
                "user" => "用户".to_owned(),
                "runtime" => "Lobotomy".to_owned(),
                other => other.strip_prefix("role:").unwrap_or(other).to_owned(),
            };
            match &m.task_id {
                Some(task) => format!("【来自 {from} · 任务 {task}】\n{}", m.body),
                None => format!("【来自 {from}】\n{}", m.body),
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnRegistered {
    pub turn_id: String,
    pub token: String,
}

/// Registers a turn for the role and binds all its queued messages (data-model.md §3.4,
/// §9.2 "登记 turn"). `continue_note` comes from the continue command, which may follow a turn
/// that did not end normally; the note opens the input, before the queued messages.
fn register(cx: &mut Cx<'_>, role: &str, continue_note: Option<&str>) -> Result<TurnRegistered> {
    let continuing = continue_note.is_some();
    let session = match current_session(cx.tx, role)? {
        Some(session) => session,
        None => start_session(cx, role)?,
    };
    if let Some(last) = last_turn(cx.tx, role)? {
        if last.state != TurnState::Ended {
            return Err(Error::rejected("turn_unfinished", format!("{role} has unfinished turn {}", last.id)));
        }
        // After an abnormal turn the role waits for the user's choice (harness-adapter.md §1.7).
        if !continuing && last.outcome != Some(Outcome::Completed) {
            return Err(Error::rejected("held", format!("{role}'s last turn {} did not complete", last.id)));
        }
    }

    let (task_id, attempt_id) = match occupant(cx.tx, role)? {
        Some(task_id) => {
            let task = load_task(cx.tx, &task_id)?;
            // Outside execution, messages to the executor wait (data-model.md §4.2).
            if task.phase != Phase::Executing {
                return Err(Error::rejected("not_executing", format!("task {} is {}", task.id, task.phase.as_str())));
            }
            if task.paused {
                return Err(Error::rejected("paused", format!("task {} is paused", task.id)));
            }
            let attempt = open_attempt(cx.tx, &task.id)?
                .ok_or_else(|| Error::rejected("no_attempt", format!("task {} has no open attempt", task.id)))?;
            // After `done` the work waits for its capture (harness-adapter.md §4.1).
            if let Some(done) = &attempt.done_turn_id {
                return Err(Error::rejected("awaiting_capture", format!("turn {done} reported done")));
            }
            (Some(task.id), Some(attempt.id))
        }
        None => (None, None),
    };

    let messages = queued_messages(cx.tx, role)?;
    let input = match (continue_note, messages.is_empty()) {
        (None, true) => {
            return Err(Error::rejected("nothing_to_deliver", format!("{role} has no queued messages")));
        }
        (None, false) => compose_input(&messages),
        (Some(note), true) => format!("【来自 Lobotomy】\n{note}"),
        (Some(note), false) => format!("【来自 Lobotomy】\n{note}\n\n{}", compose_input(&messages)),
    };
    let turn_id = new_id("turn");
    let token = new_id("tok");
    cx.tx.execute(
        "INSERT INTO turn (id, role, native_session_id, task_id, attempt_id, token, input, state, registered_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'registered', ?8)",
        params![turn_id, role, session.id, task_id, attempt_id, token, input, cx.now],
    )?;
    for m in &messages {
        cx.tx.execute("UPDATE message SET state = 'bound', turn_id = ?2 WHERE id = ?1", params![m.id, turn_id])?;
    }
    let ids: Vec<&str> = messages.iter().map(|m| m.id.as_str()).collect();
    cx.emit("turn.registered", &turn_id, json!({ "role": role, "task_id": task_id, "messages": ids }))?;
    Ok(TurnRegistered { turn_id, token })
}

fn require_turn_state(turn: &Turn, allowed: &[TurnState]) -> Result<()> {
    if allowed.contains(&turn.state) {
        return Ok(());
    }
    Err(Error::rejected("bad_turn_state", format!("turn {} is {}", turn.id, turn.state.as_str())))
}

// ---- runtime commands ----

/// The scheduler's way to start a turn when the role has queued messages.
#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterTurn {
    pub role: String,
}

impl Command for RegisterTurn {
    const NAME: &'static str = "register_turn";
    type Output = TurnRegistered;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<TurnRegistered> {
        caller.require_runtime()?;
        register(cx, &self.role, None)
    }
}

/// The CLI process exists; its pid and start time identify it for reconciliation
/// (data-model.md §3.2, §3.3).
#[derive(Debug, Serialize, Deserialize)]
pub struct TurnLaunched {
    pub turn_id: String,
    pub pid: i64,
    pub process_start: i64,
}

impl Command for TurnLaunched {
    const NAME: &'static str = "turn_launched";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let turn = load_turn(cx.tx, &self.turn_id)?;
        require_turn_state(&turn, &[TurnState::Registered])?;
        cx.tx.execute(
            "UPDATE turn SET state = 'running', pid = ?2, process_start = ?3, started_at = ?4 WHERE id = ?1",
            params![turn.id, self.pid, self.process_start, cx.now],
        )?;
        cx.emit("turn.running", &turn.id, json!({ "role": turn.role, "pid": self.pid }))?;
        Ok(())
    }
}

/// stdin was written and closed: the bound messages are delivered (data-model.md §3.4).
#[derive(Debug, Serialize, Deserialize)]
pub struct InputDelivered {
    pub turn_id: String,
}

impl Command for InputDelivered {
    const NAME: &'static str = "input_delivered";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let turn = load_turn(cx.tx, &self.turn_id)?;
        cx.tx.execute(
            "UPDATE message SET delivered_at = ?2 WHERE turn_id = ?1 AND delivered_at IS NULL",
            params![turn.id, cx.now],
        )?;
        cx.emit("turn.input_delivered", &turn.id, json!({ "role": turn.role }))?;
        Ok(())
    }
}

/// Records the harness's own session id. Codex reports it in the first turn of a session.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionIdentified {
    pub turn_id: String,
    pub native_id: String,
}

impl Command for SessionIdentified {
    const NAME: &'static str = "session_identified";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let turn = load_turn(cx.tx, &self.turn_id)?;
        match &turn.native_id {
            Some(known) if known == &self.native_id => Ok(()),
            Some(known) => Err(Error::rejected(
                "native_id_mismatch",
                format!("session {} is {known}, the harness reported {}", turn.native_session_id, self.native_id),
            )),
            None => {
                cx.tx.execute(
                    "UPDATE native_session SET native_id = ?2 WHERE id = ?1",
                    params![turn.native_session_id, self.native_id],
                )?;
                cx.emit("native_session.identified", &turn.native_session_id, json!({ "native_id": self.native_id }))?;
                Ok(())
            }
        }
    }
}

/// Ends a turn and releases the native session's run right (data-model.md §3.2). Ending an
/// ended turn again changes nothing.
#[derive(Debug, Serialize, Deserialize)]
pub struct EndTurn {
    pub turn_id: String,
    pub outcome: Outcome,
    pub failure: Option<Failure>,
}

impl Command for EndTurn {
    const NAME: &'static str = "end_turn";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let turn = load_turn(cx.tx, &self.turn_id)?;
        if turn.state == TurnState::Ended {
            return Ok(());
        }
        let failure = self.failure.as_ref().map(serde_json::to_string).transpose()?;
        cx.tx.execute(
            "UPDATE turn SET state = 'ended', outcome = ?2, failure = ?3, ended_at = ?4 WHERE id = ?1",
            params![turn.id, self.outcome.as_str(), failure, cx.now],
        )?;
        cx.emit(
            "turn.ended",
            &turn.id,
            json!({ "role": turn.role, "outcome": self.outcome, "failure": self.failure }),
        )?;
        Ok(())
    }
}

/// At startup, turns that were registered or running become unknown until reconciled
/// (data-model.md §3.2, §3.3).
#[derive(Debug, Serialize, Deserialize)]
pub struct MarkUnfinishedUnknown {}

impl Command for MarkUnfinishedUnknown {
    const NAME: &'static str = "mark_unfinished_unknown";
    type Output = Vec<String>;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<Vec<String>> {
        caller.require_runtime()?;
        let mut ids = Vec::new();
        for state in [TurnState::Registered, TurnState::Running] {
            ids.extend(turns_in_state(cx.tx, state)?.into_iter().map(|t| t.id));
        }
        for id in &ids {
            cx.tx.execute("UPDATE turn SET state = 'unknown' WHERE id = ?1", [id])?;
            cx.emit("turn.unknown", id, json!({}))?;
        }
        Ok(ids)
    }
}

// ---- user commands ----

/// Starts a turn after one that did not end normally. The input starts with a note, followed by
/// the queued messages (data-model.md §9.2 "继续"; harness-adapter.md §1.7). The runtime issues
/// it too, after a quota recovery (data-model.md §8.5).
#[derive(Debug, Serialize, Deserialize)]
pub struct Continue {
    pub request_id: String,
    pub role: String,
}

impl Command for Continue {
    const NAME: &'static str = "continue";
    type Output = TurnRegistered;

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<TurnRegistered> {
        caller.require_user_or_runtime()?;
        let last = last_turn(cx.tx, &self.role)?
            .ok_or_else(|| Error::rejected("nothing_to_continue", format!("{} has no turn", self.role)))?;
        let why = match (last.state, last.outcome) {
            (TurnState::Ended, Some(Outcome::Interrupted)) => "被中断",
            (TurnState::Ended, Some(Outcome::Failed)) => match last.failure.as_ref().map(|f| f.kind) {
                Some(FailureKind::Quota) => "因额度不足而失败",
                _ => "失败",
            },
            _ => {
                return Err(Error::rejected(
                    "nothing_to_continue",
                    format!("{}'s last turn {} is not interrupted or failed", self.role, last.id),
                ));
            }
        };
        let note = format!("上一个 turn {why}。请先检查当前工作目录的状态，再继续工作。");
        register(cx, &self.role, Some(&note))
    }
}

/// Ends the role's native session; the next turn starts a fresh one. If the role is executing a
/// task, the brief is queued again, because the new session has not seen it (data-model.md §4.1).
#[derive(Debug, Serialize, Deserialize)]
pub struct NewNativeSession {
    pub request_id: String,
    pub role: String,
}

impl Command for NewNativeSession {
    const NAME: &'static str = "new_native_session";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user()?;
        role_harness(cx.tx, &self.role)?;
        if let Some(last) = last_turn(cx.tx, &self.role)?
            && last.state != TurnState::Ended
        {
            return Err(Error::rejected("turn_unfinished", format!("{} has unfinished turn {}", self.role, last.id)));
        }
        if let Some(session) = current_session(cx.tx, &self.role)? {
            cx.tx.execute("UPDATE native_session SET ended_at = ?2 WHERE id = ?1", params![session.id, cx.now])?;
            cx.emit("native_session.ended", &session.id, json!({ "role": self.role }))?;
        }
        start_session(cx, &self.role)?;
        if let Some(task_id) = occupant(cx.tx, &self.role)? {
            let task = load_task(cx.tx, &task_id)?;
            if let Some(attempt) = task::open_attempt(cx.tx, &task.id)? {
                let body = format!("（新会话）{}", brief(cx.tx, &task, attempt.seq)?);
                queue_message(cx, &self.role, &Caller::Runtime, Some(&task.id), &body)?;
            }
        }
        Ok(())
    }
}
