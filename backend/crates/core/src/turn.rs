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
use crate::prompts;
use crate::sql::sql_enum;
use crate::task::{
    Attempt, Message, Phase, Task, attempt_brief, load_task, occupant, open_attempt, queue_message, queued_messages,
};

sql_enum! {
    pub enum TurnState {
        /// The turn record exists; the CLI has not started.
        Registered = "registered",
        Running = "running",
        Ended = "ended",
        /// The backend restarted while the turn was registered or running (data-model.md §3.3).
        Unknown = "unknown",
    }
}

sql_enum! {
    pub enum Outcome {
        Completed = "completed",
        Failed = "failed",
        Interrupted = "interrupted",
    }
}

/// Why a turn failed (data-model.md §3.2, §8.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
    /// When the quota resets, in Unix milliseconds, if the harness said so.
    pub resets_at: Option<i64>,
    /// The CLI exited without writing anything on stdout: the harness never began the turn, and
    /// its input reached no session. Continuing sends the input again (data-model.md §3.4).
    #[serde(default)]
    pub unstarted: bool,
}

impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), resets_at: None, unstarted: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Quota,
    /// The environment does not allow the harness's permission mode (harness-adapter.md §1.9).
    Permission,
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
    /// The turn's MCP token: only its CLI needs it, so it is never serialized, to the GUI or
    /// anywhere else (#16).
    #[serde(skip)]
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
    /// The slot generation the turn runs in (harness-adapter.md §3).
    pub workspace_id: Option<String>,
}

const TURN_SELECT: &str = "SELECT t.id, t.role, s.harness, t.native_session_id, s.native_id, t.task_id, t.attempt_id,
       t.token, t.input, t.state, t.outcome, t.failure, t.pid, t.process_start, t.done_at,
       t.registered_at, t.started_at, t.ended_at, t.workspace_id
     FROM turn t JOIN native_session s ON s.id = t.native_session_id";

fn turn_from_row(r: &Row<'_>) -> Result<Turn> {
    Ok(Turn {
        id: r.get(0)?,
        role: r.get(1)?,
        harness: r.get(2)?,
        native_session_id: r.get(3)?,
        native_id: r.get(4)?,
        task_id: r.get(5)?,
        attempt_id: r.get(6)?,
        token: r.get(7)?,
        input: r.get(8)?,
        state: r.get(9)?,
        outcome: r.get(10)?,
        failure: r.get::<_, Option<String>>(11)?.as_deref().map(serde_json::from_str).transpose()?,
        pid: r.get(12)?,
        process_start: r.get(13)?,
        done_at: r.get(14)?,
        registered_at: r.get(15)?,
        started_at: r.get(16)?,
        ended_at: r.get(17)?,
        workspace_id: r.get(18)?,
    })
}

fn query_turns(conn: &Connection, filter: &str, args: impl rusqlite::Params) -> Result<Vec<Turn>> {
    let mut stmt = conn.prepare(&format!("{TURN_SELECT} {filter}"))?;
    stmt.query_and_then(args, turn_from_row)?.collect()
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

/// The latest turn of the role, in any session.
pub fn last_turn(conn: &Connection, role: &str) -> Result<Option<Turn>> {
    Ok(query_turns(conn, "WHERE t.role = ?1 ORDER BY t.registered_at DESC, t.id DESC LIMIT 1", [role])?.pop())
}

/// The latest turn of a native session.
pub fn last_turn_in(conn: &Connection, session_id: &str) -> Result<Option<Turn>> {
    Ok(query_turns(
        conn,
        "WHERE t.native_session_id = ?1 ORDER BY t.registered_at DESC, t.id DESC LIMIT 1",
        [session_id],
    )?
    .pop())
}

/// The role's unfinished turn. A role has at most one, whatever the session, because it has one
/// slot (data-model.md §2).
pub fn unfinished_turn(conn: &Connection, role: &str) -> Result<Option<Turn>> {
    Ok(query_turns(conn, "WHERE t.role = ?1 AND t.state <> 'ended' LIMIT 1", [role])?.pop())
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NativeSession {
    pub id: String,
    pub role: String,
    pub harness: String,
    /// The task the session belongs to; `None` for messages outside any task.
    pub task_id: Option<String>,
    pub native_id: Option<String>,
}

/// The role's current session for a task, or for work outside any task when `task_id` is `None`.
/// Each task gets a fresh session; it ends when the task closes (#11).
pub fn current_session(conn: &Connection, role: &str, task_id: Option<&str>) -> Result<Option<NativeSession>> {
    Ok(conn
        .query_row(
            "SELECT id, role, harness, task_id, native_id FROM native_session
             WHERE role = ?1 AND task_id IS ?2 AND ended_at IS NULL",
            params![role, task_id],
            |r| {
                Ok(NativeSession {
                    id: r.get(0)?,
                    role: r.get(1)?,
                    harness: r.get(2)?,
                    task_id: r.get(3)?,
                    native_id: r.get(4)?,
                })
            },
        )
        .optional()?)
}

fn role_harness(conn: &Connection, role: &str) -> Result<String> {
    conn.query_row("SELECT harness FROM role WHERE name = ?1", [role], |r| r.get(0))
        .optional()?
        .ok_or_else(|| Error::rejected("unknown_role", format!("no role {role}")))
}

fn start_session(cx: &mut Cx<'_>, role: &str, task_id: Option<&str>) -> Result<NativeSession> {
    let harness = role_harness(cx.tx, role)?;
    let id = new_id("ns");
    cx.tx.execute(
        "INSERT INTO native_session (id, role, harness, task_id, started_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, role, harness, task_id, cx.now],
    )?;
    cx.emit("native_session.started", &id, json!({ "role": role, "task_id": task_id }))?;
    Ok(NativeSession { id, role: role.to_owned(), harness, task_id: task_id.map(str::to_owned), native_id: None })
}

/// Ends the sessions of a closing task (#11). A turn still running in one of them runs to its
/// end; its late reports change nothing.
pub(crate) fn end_task_sessions(cx: &mut Cx<'_>, task_id: &str) -> Result<()> {
    let ids: Vec<String> = {
        let mut stmt = cx.tx.prepare("SELECT id FROM native_session WHERE task_id = ?1 AND ended_at IS NULL")?;
        stmt.query_map([task_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    for id in &ids {
        cx.tx.execute("UPDATE native_session SET ended_at = ?2 WHERE id = ?1", params![id, cx.now])?;
        cx.emit("native_session.ended", id, json!({ "task_id": task_id }))?;
    }
    Ok(())
}

/// What the role works on: the occupying task and its open attempt, or nothing.
fn current_work(conn: &Connection, role: &str) -> Result<(Option<Task>, Option<Attempt>)> {
    let Some(task_id) = occupant(conn, role)? else {
        return Ok((None, None));
    };
    let task = load_task(conn, &task_id)?;
    let attempt = open_attempt(conn, &task.id)?;
    Ok((Some(task), attempt))
}

fn is_abnormal(turn: &Turn) -> bool {
    turn.state == TurnState::Ended && matches!(turn.outcome, Some(Outcome::Interrupted | Outcome::Failed))
}

/// The text written to the CLI's stdin: the bound messages in arrival order.
pub fn compose_input(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|m| format!("{}\n{}", prompts::header(prompts::sender(&m.source), m.task_id.as_deref()), m.body))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Why a role waits for the user.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Hold {
    /// The last turn of the current attempt was interrupted or failed (harness-adapter.md §1.7).
    Abnormal { turn: Box<Turn> },
    /// The runtime stopped the latest capture (harness-adapter.md §4.1).
    CaptureStopped { capture: Box<crate::capture::Capture> },
    /// A completed turn left no harness session id; only a new native session helps (#10).
    SessionUnidentified { session_id: String },
}

impl Hold {
    /// The code [`register`] refuses with while the role holds.
    fn code(&self) -> &'static str {
        match self {
            Hold::Abnormal { .. } => "held",
            Hold::CaptureStopped { .. } => "capture_stopped",
            Hold::SessionUnidentified { .. } => "session_unidentified",
        }
    }
}

/// What keeps a role from its next turn until someone acts.
enum Waiting {
    /// The runtime: the last turn's scene is not captured yet (harness-adapter.md §4.1). The
    /// capture's id.
    Runtime(String),
    /// The user.
    User(Hold),
}

/// The one place that decides whether a role waits. [`hold`], [`register`] and [`Continue`] all
/// ask here, so what the GUI shows and what the runtime refuses cannot drift apart (#16).
fn waiting(conn: &Connection, role: &str) -> Result<Option<Waiting>> {
    if let Some(capture) = crate::capture::capture_in_progress(conn, role)? {
        return Ok(Some(Waiting::Runtime(capture.id)));
    }
    if let Some(capture) = crate::capture::stopped_capture(conn, role)? {
        return Ok(Some(Waiting::User(Hold::CaptureStopped { capture: Box::new(capture) })));
    }
    let (task, attempt) = current_work(conn, role)?;
    let Some(session) = current_session(conn, role, task.as_ref().map(|t| t.id.as_str()))? else {
        return Ok(None);
    };
    if let Some(turn) = abnormal_last_turn(conn, &session, attempt.as_ref().map(|a| a.id.as_str()))? {
        return Ok(Some(Waiting::User(Hold::Abnormal { turn: Box::new(turn) })));
    }
    if lost_session_id(conn, &session)? {
        return Ok(Some(Waiting::User(Hold::SessionUnidentified { session_id: session.id })));
    }
    Ok(None)
}

/// The session's last turn, if it belongs to the current attempt and did not end normally. A turn
/// of an attempt that has ended does not hold: the capture or the user has already moved the work
/// on (#11).
fn abnormal_last_turn(conn: &Connection, session: &NativeSession, attempt_id: Option<&str>) -> Result<Option<Turn>> {
    Ok(last_turn_in(conn, &session.id)?.filter(|last| is_abnormal(last) && last.attempt_id.as_deref() == attempt_id))
}

/// A completed turn without a recorded session id: the next turn would start a fresh harness
/// session and silently lose the context (#10).
fn lost_session_id(conn: &Connection, session: &NativeSession) -> Result<bool> {
    if session.native_id.is_some() {
        return Ok(false);
    }
    Ok(conn
        .query_row(
            "SELECT 1 FROM turn WHERE native_session_id = ?1 AND outcome = 'completed' LIMIT 1",
            [&session.id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Why the role waits for the user, if it does. While the last turn's scene is being captured it
/// waits for the runtime, which is not a hold.
pub fn hold(conn: &Connection, role: &str) -> Result<Option<Hold>> {
    Ok(match waiting(conn, role)? {
        Some(Waiting::User(hold)) => Some(hold),
        Some(Waiting::Runtime(_)) | None => None,
    })
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnRegistered {
    pub turn_id: String,
    pub token: String,
}

/// What the continue command adds to the turn it registers (data-model.md §9.2 "继续").
struct Continuation<'a> {
    /// Opens the input.
    note: Option<String>,
    /// A failed turn the harness never began: its input goes again, after the note, and its
    /// messages move to the new turn (data-model.md §3.4).
    resend: Option<&'a Turn>,
}

/// Registers a turn for the role and binds all its queued messages (data-model.md §3.4,
/// §9.2 "登记 turn"). `continuation` comes from the continue command, which may follow a turn
/// that did not end normally; what it adds comes before the queued messages.
fn register(cx: &mut Cx<'_>, role: &str, continuation: Option<Continuation<'_>>) -> Result<TurnRegistered> {
    if let Some(running) = unfinished_turn(cx.tx, role)? {
        return Err(Error::rejected("turn_unfinished", format!("{role} has unfinished turn {}", running.id)));
    }
    // The last turn's scene is captured before the next turn touches the slot (harness-adapter.md
    // §4.1). A role that holds waits for the user's choice; continuing is one of the choices, and
    // gets past an abnormal turn or a stopped capture, never past a lost session id.
    match waiting(cx.tx, role)? {
        Some(Waiting::Runtime(capture)) => {
            return Err(Error::rejected("capture_pending", format!("capture {capture} of {role} is pending")));
        }
        Some(Waiting::User(hold)) if continuation.is_none() => {
            return Err(Error::rejected(hold.code(), format!("{role} waits for the user: {}", hold.code())));
        }
        _ => {}
    }
    // Turns only run in a slot that holds its target (harness-adapter.md §3).
    let workspace_id = match crate::workspace::role_slot(cx.tx, role)? {
        Some(slot) => match crate::workspace::current_workspace(cx.tx, &slot)? {
            Some(ws) if ws.state == crate::workspace::WorkspaceState::Ready => Some(ws.id),
            _ => return Err(Error::rejected("slot_not_ready", format!("slot {slot} of {role} is not ready"))),
        },
        None => None,
    };
    let (task, attempt) = current_work(cx.tx, role)?;
    if let Some(task) = &task {
        // Outside execution, messages to the executor wait (data-model.md §4.2).
        if task.phase != Phase::Executing {
            return Err(Error::rejected("not_executing", format!("task {} is {}", task.id, task.phase.as_str())));
        }
        if task.paused {
            return Err(Error::rejected("paused", format!("task {} is paused", task.id)));
        }
        if attempt.is_none() {
            return Err(Error::rejected("no_attempt", format!("task {} has no open attempt", task.id)));
        }
    }
    let task_id = task.as_ref().map(|t| t.id.clone());
    let attempt_id = attempt.map(|a| a.id);

    let session = match current_session(cx.tx, role, task_id.as_deref())? {
        Some(session) => session,
        None => start_session(cx, role, task_id.as_deref())?,
    };
    // Also when continuing, which gets past other holds.
    if lost_session_id(cx.tx, &session)? {
        return Err(Error::rejected("session_unidentified", format!("session {} has no harness id", session.id)));
    }

    let messages = queued_messages(cx.tx, role)?;
    let (note, resend) = continuation.map_or((None, None), |c| (c.note, c.resend));
    let mut parts = Vec::new();
    if let Some(note) = note {
        parts.push(format!("{}\n{note}", prompts::header("Lobotomy", None)));
    }
    if let Some(turn) = resend {
        parts.push(turn.input.clone());
    }
    if !messages.is_empty() {
        parts.push(compose_input(&messages));
    }
    if parts.is_empty() {
        return Err(Error::rejected("nothing_to_deliver", format!("{role} has no queued messages")));
    }
    let input = parts.join("\n\n");
    let turn_id = new_id("turn");
    let token = new_id("tok");
    cx.tx.execute(
        "INSERT INTO turn (id, role, native_session_id, task_id, attempt_id, token, input, state, registered_at,
                           workspace_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'registered', ?8, ?9)",
        params![turn_id, role, session.id, task_id, attempt_id, token, input, cx.now, workspace_id],
    )?;
    let mut ids: Vec<String> = Vec::new();
    if let Some(turn) = resend {
        let mut stmt = cx.tx.prepare("SELECT id FROM message WHERE turn_id = ?1 ORDER BY seq")?;
        ids = stmt.query_map([&turn.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        cx.tx.execute(
            "UPDATE message SET turn_id = ?2, delivered_at = NULL WHERE turn_id = ?1",
            params![turn.id, turn_id],
        )?;
    }
    for m in &messages {
        cx.tx.execute("UPDATE message SET state = 'bound', turn_id = ?2 WHERE id = ?1", params![m.id, turn_id])?;
    }
    // The executor's question stands until a message from the user goes to it. Once the message is
    // bound, the executor gets it; if it still needs an answer, it asks again (#17).
    if let Some(task) = &task
        && task.blocked_reason.is_some()
        && messages.iter().any(|m| m.source == Caller::User.scope())
    {
        crate::report::set_blocked(cx, task, None)?;
    }
    ids.extend(messages.iter().map(|m| m.id.clone()));
    let resent = resend.map(|t| t.id.as_str());
    cx.emit(
        "turn.registered",
        &turn_id,
        json!({ "role": role, "task_id": task_id, "messages": ids, "resent": resent }),
    )?;
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

/// Ends a turn, releases the role's run right and records the intent to capture the slot
/// (data-model.md §3.2; harness-adapter.md §4.1). Ending an ended turn again changes nothing.
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
        crate::capture::intend(cx, &turn, self.outcome)?;
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

/// Starts a turn after one of the current attempt that did not end normally, or after a capture
/// the runtime stopped. The input starts with a note, followed by the queued messages
/// (data-model.md §9.2 "继续"; harness-adapter.md §1.7, §4.1). After a turn the harness never
/// began, its input goes again instead of the note (data-model.md §3.4). Only the user issues it;
/// the runtime never continues on its own, quota failures included (data-model.md §8.5).
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
        caller.require_user()?;
        // Only an abnormal turn of the current work can be continued (#11).
        let (task, attempt) = current_work(cx.tx, &self.role)?;
        let session = current_session(cx.tx, &self.role, task.as_ref().map(|t| t.id.as_str()))?;
        let attempt_id = attempt.map(|a| a.id);
        let last = match &session {
            Some(session) => abnormal_last_turn(cx.tx, session, attempt_id.as_deref())?,
            None => None,
        };
        let stopped = crate::capture::stopped_capture(cx.tx, &self.role)?;
        if last.is_none() && stopped.is_none() {
            return Err(Error::rejected("nothing_to_continue", format!("{} has no turn to continue", self.role)));
        }
        // A turn the harness never began changed nothing and told the role nothing: its input
        // goes again as it was, with no note about it.
        let resend = last.as_ref().filter(|last| last.failure.as_ref().is_some_and(|f| f.unstarted));
        let mut notes = Vec::new();
        if let Some(last) = &last
            && resend.is_none()
        {
            let interrupted = last.outcome == Some(Outcome::Interrupted);
            notes.push(prompts::continue_note(interrupted, last.failure.as_ref().map(|f| f.kind)));
        }
        if let Some(stopped) = &stopped {
            notes.push(crate::capture::stopped_note(stopped));
            // The executor deals with the list, so the stopped done no longer counts.
            if let Some(attempt_id) = &attempt_id {
                cx.tx.execute(
                    "UPDATE attempt SET done_turn_id = NULL, done_summary = NULL, trial = NULL
                     WHERE id = ?1 AND done_turn_id = ?2",
                    params![attempt_id, stopped.turn_id],
                )?;
            }
        }
        let note = (!notes.is_empty()).then(|| notes.join("\n\n"));
        register(cx, &self.role, Some(Continuation { note, resend }))
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
        require_no_unfinished_turn(cx, &self.role)?;
        end_current_session(cx, &self.role)?;
        let task = occupant(cx.tx, &self.role)?;
        start_session(cx, &self.role, task.as_deref())?;
        queue_brief_again(cx, &self.role)
    }
}

fn require_no_unfinished_turn(cx: &Cx<'_>, role: &str) -> Result<()> {
    match unfinished_turn(cx.tx, role)? {
        Some(running) => Err(Error::rejected("turn_unfinished", format!("{role} has unfinished turn {}", running.id))),
        None => Ok(()),
    }
}

/// Ends the role's session for its current work. Whether it had one.
fn end_current_session(cx: &mut Cx<'_>, role: &str) -> Result<bool> {
    let task = occupant(cx.tx, role)?;
    let Some(session) = current_session(cx.tx, role, task.as_deref())? else { return Ok(false) };
    cx.tx.execute("UPDATE native_session SET ended_at = ?2 WHERE id = ?1", params![session.id, cx.now])?;
    cx.emit("native_session.ended", &session.id, json!({ "role": role }))?;
    Ok(true)
}

/// A new session has not seen the attempt's brief, so it is queued again (data-model.md §4.1).
fn queue_brief_again(cx: &mut Cx<'_>, role: &str) -> Result<()> {
    if let (Some(task), Some(attempt)) = current_work(cx.tx, role)? {
        let body = format!("{}{}", prompts::NEW_SESSION, attempt_brief(cx.tx, &task, attempt.seq, &attempt.conflicts)?);
        queue_message(cx, role, &Caller::Runtime, Some(&task.id), &body)?;
    }
    Ok(())
}

/// Puts the role on another harness: no role is bound to one (harness-adapter.md §0). A native
/// session belongs to its harness, so the role's current session ends, and its next turn starts
/// a new one with the brief, as after a new native session. Choosing the harness the role is on
/// changes nothing.
#[derive(Debug, Serialize, Deserialize)]
pub struct SetRoleHarness {
    pub request_id: String,
    pub role: String,
    pub harness: String,
}

impl Command for SetRoleHarness {
    const NAME: &'static str = "set_role_harness";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user()?;
        if !crate::role::HARNESSES.contains(&self.harness.as_str()) {
            return Err(Error::rejected("unknown_harness", format!("unknown harness {}", self.harness)));
        }
        if role_harness(cx.tx, &self.role)? == self.harness {
            return Ok(());
        }
        require_no_unfinished_turn(cx, &self.role)?;
        cx.tx.execute("UPDATE role SET harness = ?2 WHERE name = ?1", params![self.role, self.harness])?;
        cx.emit("role.harness_changed", &self.role, json!({ "harness": self.harness }))?;
        // Without a session, the brief has not left the inbox yet.
        if end_current_session(cx, &self.role)? {
            queue_brief_again(cx, &self.role)?;
        }
        Ok(())
    }
}
