//! Question lifecycle shared by worker reports and Angela's ask_user. The boundary is the
//! inbox's durable sequence at the asking transaction, never a wall-clock comparison.

use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::json;

use crate::command::Cx;
use crate::error::Result;
use crate::id::new_id;
use crate::task::Message;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Question {
    pub id: String,
    pub body: String,
    pub created_at: i64,
    pub role: String,
    pub task_id: Option<String>,
    pub turn_id: String,
    pub after_message_seq: i64,
}

impl Question {
    /// A subsequent message is a candidate answer, not proof that its words resolve the issue.
    /// Role-wide messages can answer the role's task question; another task's messages cannot.
    pub fn accepts(&self, message: &Message) -> bool {
        message.role == self.role
            && message.seq > self.after_message_seq
            && (message.source == "user" || (self.task_id.is_some() && message.source == "manager"))
            && (message.task_id.is_none() || message.task_id == self.task_id)
    }
}

/// All current questions, optionally restricted to the role receiving a turn.
pub fn open_questions(conn: &Connection, role: Option<&str>) -> Result<Vec<Question>> {
    let mut stmt = conn.prepare(
        "SELECT id, body, created_at, role, task_id, turn_id, after_message_seq FROM question
         WHERE closed_at IS NULL AND (?1 IS NULL OR role = ?1) ORDER BY role, rowid",
    )?;
    let rows = stmt.query_map([role], |r| {
        Ok(Question {
            id: r.get(0)?,
            body: r.get(1)?,
            created_at: r.get(2)?,
            role: r.get(3)?,
            task_id: r.get(4)?,
            turn_id: r.get(5)?,
            after_message_seq: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Returns the id and whether a new question was asked. All callers share the duplicate rule.
pub(crate) fn ask(
    cx: &mut Cx<'_>,
    role: &str,
    task_id: Option<&str>,
    turn_id: &str,
    body: &str,
) -> Result<(String, bool)> {
    if let Some(question) = open_questions(cx.tx, Some(role))?
        .into_iter()
        .find(|q| q.task_id.as_deref() == task_id && q.turn_id == turn_id && q.body == body)
    {
        return Ok((question.id, false));
    }
    if let Some(task) = task_id {
        retire_task(cx, task)?;
    }
    let id = new_id("q");
    cx.tx.execute(
        "INSERT INTO question (id, body, turn_id, created_at, role, task_id, after_message_seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, (SELECT COALESCE(MAX(seq), 0) FROM message))",
        params![id, body, turn_id, cx.now, role, task_id],
    )?;
    if let Some(task) = task_id {
        task_changed(cx, task, Some(body))?;
    }
    cx.emit("question.asked", &id, json!({ "role": role, "task_id": task_id }))?;
    Ok((id, true))
}

/// Retires a task's question during a phase transition or replacement. The enclosing command
/// updates the task revision and emits its final state, not an intermediate withdrawal.
pub(crate) fn retire_task(cx: &Cx<'_>, task_id: &str) -> Result<usize> {
    Ok(cx.tx.execute(
        "UPDATE question SET closed_at = ?2 WHERE task_id = ?1 AND closed_at IS NULL",
        params![task_id, cx.now],
    )?)
}

/// Progress/done withdraw the current question without claiming that a message answered it.
pub(crate) fn withdraw_task(cx: &mut Cx<'_>, task_id: &str) -> Result<()> {
    if retire_task(cx, task_id)? > 0 {
        task_changed(cx, task_id, None)?;
    }
    Ok(())
}

/// Called with the same questions and messages used to compose this turn's input.
pub(crate) fn close_answered(
    cx: &mut Cx<'_>,
    turn_id: &str,
    questions: &[Question],
    messages: &[Message],
) -> Result<()> {
    for q in questions {
        let answers: Vec<&str> = messages.iter().filter(|m| q.accepts(m)).map(|m| m.id.as_str()).collect();
        if answers.is_empty() {
            continue;
        }
        cx.tx.execute(
            "UPDATE question SET closed_at = ?2, closed_in = ?3 WHERE id = ?1 AND closed_at IS NULL",
            params![q.id, cx.now, turn_id],
        )?;
        if let Some(task_id) = &q.task_id {
            task_changed(cx, task_id, None)?;
        }
        cx.emit("question.closed", &q.id, json!({ "turn_id": turn_id, "messages": answers }))?;
    }
    Ok(())
}

/// Preserve task revision/event contracts without storing a second copy of the question.
fn task_changed(cx: &mut Cx<'_>, task_id: &str, reason: Option<&str>) -> Result<()> {
    cx.tx.execute("UPDATE task SET revision = revision + 1 WHERE id = ?1", [task_id])?;
    cx.emit("task.blocked_changed", task_id, json!({ "blocked_reason": reason }))?;
    Ok(())
}
