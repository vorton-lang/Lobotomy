//! The manager's records in her own database (data-model.md §10.6): how far she has read each
//! project, the runtime's messages in her inbox about what changed there (manager-actions.md §6
//! "Manager 什么时候运行"), and her questions to the user (`ask_user`).

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::id::new_id;
use crate::task::queue_message;
use crate::turn::{TurnState, load_turn};

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

/// A question the manager asked the user, open until a message of the user's sent after it is
/// delivered to her (manager-actions.md §6 `ask_user`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Question {
    pub id: String,
    pub body: String,
    pub created_at: i64,
}

pub fn open_questions(conn: &Connection) -> Result<Vec<Question>> {
    let mut stmt =
        conn.prepare("SELECT id, body, created_at FROM question WHERE closed_at IS NULL ORDER BY created_at, id")?;
    let rows = stmt.query_map([], |r| Ok(Question { id: r.get(0)?, body: r.get(1)?, created_at: r.get(2)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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
        if load_turn(cx.tx, turn_id)?.state == TurnState::Ended {
            return Err(Error::rejected("turn_ended", format!("turn {turn_id} has ended")));
        }
        if self.question.trim().is_empty() {
            return Err(Error::rejected("missing_field", "a question needs words"));
        }
        let id = new_id("q");
        cx.tx.execute(
            "INSERT INTO question (id, body, turn_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, self.question, turn_id, cx.now],
        )?;
        cx.emit("question.asked", &id, json!({}))?;
        Ok(id)
    }
}

/// The user's message delivered in `turn_id` answers the questions asked before it was sent. If
/// one still needs an answer, she asks again (as for an executor's question, #17).
pub(crate) fn close_answered(cx: &mut Cx<'_>, turn_id: &str, user_sent_at: i64) -> Result<()> {
    let closed = cx.tx.execute(
        "UPDATE question SET closed_at = ?1, closed_in = ?2 WHERE closed_at IS NULL AND created_at <= ?3",
        params![cx.now, turn_id, user_sent_at],
    )?;
    if closed > 0 {
        cx.emit("question.closed", turn_id, json!({ "questions": closed }))?;
    }
    Ok(())
}
