//! The manager's records in her own database (data-model.md §10.6): how far she has read each
//! project, the runtime's messages in her inbox about what changed there (manager-actions.md §6
//! "Manager 什么时候运行"), and her questions to the user (`ask_user`).

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::task::queue_message;
use crate::turn::{RegisterTurn, TurnRegistered, TurnState, hold, load_turn};
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

/// Commits a scheduler read of the manager's inbox and session. Cross-project reads happen
/// before this command; her own state must still match that read. Notices, cursors and turn
/// registration commit together, or a refusal rolls all of them back.
#[derive(Debug, Serialize, Deserialize)]
pub struct ScheduleManagerTurn {
    pub role: String,
    pub as_of: i64,
    pub messages: Vec<String>,
    pub cursors: Vec<Cursor>,
    /// This read found input that may start a turn. A held role still receives its notices.
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
        for c in &self.cursors {
            cx.tx.execute(
                "INSERT INTO project_cursor (project_id, seq) VALUES (?1, ?2)
                 ON CONFLICT (project_id) DO UPDATE SET seq = excluded.seq",
                params![c.project_id, c.seq],
            )?;
        }
        // System notices remain visible while the role waits for the user. Only a user action
        // can release that hold; the ordinary turn command still checks every other condition.
        if self.register && hold(cx.tx, &self.role)?.is_none() {
            RegisterTurn { role: self.role.clone() }.apply(caller, cx).map(Some)
        } else {
            Ok(None)
        }
    }
}

pub use crate::question::{Question, QuestionKind};

/// Angela's open questions. Queued candidate replies hide attention but do not close them.
pub fn open_questions(conn: &Connection) -> Result<Vec<Question>> {
    Ok(crate::question::open_questions(conn, None)?.into_iter().filter(|q| q.task_id.is_none()).collect())
}

/// The manager asks the user something that only the user decides. It is a state, not a
/// notification: the question waits in "等你决定" until the user answers (manager-actions.md §6:
/// she expresses urgency only by changing state). Tasks that must stop meanwhile she pauses
/// herself, in their project: a command writes one database (data-model.md §10.6 跨库操作第 5 条).
/// The options are the card's buttons (tech-plan.md §5 "要你卡"); the user may always write instead.
#[derive(Debug, Serialize, Deserialize)]
pub struct AskUser {
    pub question: String,
    pub kind: QuestionKind,
    #[serde(default)]
    pub options: Vec<String>,
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
        let options: Vec<String> = self.options.iter().map(|o| o.trim().to_owned()).collect();
        if options.iter().any(String::is_empty) {
            return Err(Error::rejected("missing_field", "每个选项都要有字"));
        }
        let fewest = match self.kind {
            QuestionKind::Confirm => 1,
            QuestionKind::Choice => 2,
            QuestionKind::Open => 0,
        };
        if options.len() < fewest {
            return Err(Error::rejected("missing_field", format!("{} 至少要 {fewest} 个选项", self.kind.as_str())));
        }
        if self.kind == QuestionKind::Open && !options.is_empty() {
            return Err(Error::rejected("invalid_field", "open 是直接回答的问题，不带选项；要给选项就用 choice"));
        }
        crate::question::ask(cx, &turn.role, None, turn_id, &self.question, self.kind, &options).map(|(id, _)| id)
    }
}
