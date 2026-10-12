//! Question lifecycle shared by worker reports and Angela's ask_user. The boundary is the
//! inbox's durable sequence at the asking transaction, never a wall-clock comparison.

use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::json;

use crate::command::Cx;
use crate::error::Result;
use crate::id::new_id;
use crate::task::{Message, Task, load_task, queued_messages};

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

    pub fn reply_queued(&self, conn: &Connection) -> Result<bool> {
        Ok(queued_messages(conn, &self.role)?.iter().any(|m| self.accepts(m)))
    }
}

pub fn open_questions(conn: &Connection, role: &str) -> Result<Vec<Question>> {
    let mut stmt = conn.prepare(
        "SELECT id, body, created_at, role, task_id, turn_id, after_message_seq FROM question
         WHERE role = ?1 AND closed_at IS NULL ORDER BY rowid",
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

pub(crate) fn ask(cx: &mut Cx<'_>, role: &str, task_id: Option<&str>, turn_id: &str, body: &str) -> Result<String> {
    if let Some(question) = open_questions(cx.tx, role)?
        .into_iter()
        .find(|q| q.task_id.as_deref() == task_id && q.turn_id == turn_id && q.body == body)
    {
        return Ok(question.id);
    }
    let id = new_id("q");
    cx.tx.execute(
        "INSERT INTO question (id, body, turn_id, created_at, role, task_id, after_message_seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, (SELECT COALESCE(MAX(seq), 0) FROM message))",
        params![id, body, turn_id, cx.now, role, task_id],
    )?;
    cx.emit("question.asked", &id, json!({ "role": role, "task_id": task_id }))?;
    Ok(id)
}

/// Superseding, withdrawing or leaving an attempt retires its question without claiming an answer.
pub(crate) fn close_task(cx: &Cx<'_>, task_id: &str) -> Result<()> {
    cx.tx.execute(
        "UPDATE question SET closed_at = ?2 WHERE task_id = ?1 AND closed_at IS NULL",
        params![task_id, cx.now],
    )?;
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
            set_task_question(cx, &load_task(cx.tx, task_id)?, None, turn_id)?;
        }
        cx.emit("question.closed", &q.id, json!({ "turn_id": turn_id, "messages": answers }))?;
    }
    Ok(())
}

pub(crate) fn set_task_question(cx: &mut Cx<'_>, task: &Task, reason: Option<&str>, turn_id: &str) -> Result<()> {
    if reason.is_none() && task.blocked_reason.is_none() {
        return Ok(());
    }
    close_task(cx, &task.id)?;
    if let Some(reason) = reason {
        ask(cx, &task.executor, Some(&task.id), turn_id, reason)?;
    }
    cx.tx.execute(
        "UPDATE task SET blocked_reason = ?2, revision = revision + 1 WHERE id = ?1",
        params![task.id, reason],
    )?;
    cx.emit("task.blocked_changed", &task.id, json!({ "blocked_reason": reason }))?;
    Ok(())
}
