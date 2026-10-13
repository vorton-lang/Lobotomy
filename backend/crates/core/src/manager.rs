//! The manager's records in her own database (data-model.md §10.6): how far she has read each
//! project, the runtime's messages in her inbox about what changed there (manager-actions.md §6
//! "Manager 什么时候运行"), and her questions to the user (`ask_user`).

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::task::queue_message;
use crate::turn::{RegisterTurn, TurnRegistered, TurnState, load_turn};
use crate::view::last_event;

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

/// Commits a scheduler read of the manager's inbox and session. Cross-project reads happen
/// before this command; her own state must still match that read. Notices, cursors and turn
/// registration commit together, or a refusal rolls all of them back.
#[derive(Debug, Serialize, Deserialize)]
pub struct ScheduleManagerTurn {
    pub role: String,
    pub as_of: i64,
    pub messages: Vec<String>,
    pub cursors: Vec<Cursor>,
    /// Idle reads only initialize cursors; held turns receive notices without consuming messages.
    pub register: bool,
}

impl Command for ScheduleManagerTurn {
    const NAME: &'static str = "schedule_manager_turn";
    type Output = Option<TurnRegistered>;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<Self::Output> {
        caller.require_runtime()?;
        if last_event(cx.tx)? != self.as_of {
            return Err(Error::rejected("manager_state_changed", "the manager changed while preparing her input"));
        }
        for body in &self.messages {
            queue_message(cx, &self.role, caller, None, body)?;
        }
        TellManager { role: self.role.clone(), body: None, cursors: self.cursors.clone() }.apply(caller, cx)?;
        if self.register { RegisterTurn { role: self.role.clone() }.apply(caller, cx).map(Some) } else { Ok(None) }
    }
}

pub use crate::question::Question;

/// Angela's open questions. Queued candidate replies hide attention but do not close them.
pub fn open_questions(conn: &Connection) -> Result<Vec<Question>> {
    Ok(crate::question::open_questions(conn, None)?.into_iter().filter(|q| q.task_id.is_none()).collect())
}

/// The manager asks the user something that only the user decides. It is a state, not a
/// notification: the question waits in "等你决定" until the user answers (manager-actions.md §6:
/// she expresses urgency only by changing state). Tasks that must stop meanwhile she pauses
/// herself, in their project: a command writes one database (data-model.md §10.6 跨库操作第 5 条).
#[derive(Debug, Serialize, Deserialize)]
pub struct AskUser {
    pub question: String,
}

impl Command for AskUser {
    const NAME: &'static str = "ask_user";
    type Output = String;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<String> {
        let Caller::Role { turn_id, .. } = caller else {
            return Err(Error::rejected("forbidden", format!("{} may not ask the user", caller.scope())));
        };
        let turn = load_turn(cx.tx, turn_id)?;
        if turn.state == TurnState::Ended {
            return Err(Error::rejected("turn_ended", format!("turn {turn_id} has ended")));
        }
        if self.question.trim().is_empty() {
            return Err(Error::rejected("missing_field", "a question needs words"));
        }
        crate::question::ask(cx, &turn.role, None, turn_id, &self.question).map(|(id, _)| id)
    }
}
