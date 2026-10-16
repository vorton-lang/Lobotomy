//! `org_report`: how an executor reports progress, a blocking question or `done`
//! (roles-and-tasks.md §4; data-model.md §9.2).

use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::question;
use crate::task::{Phase, Task, load_task, open_attempt};
use crate::turn::{Turn, TurnState, load_turn};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum ReportStatus {
    Progress,
    Blocked,
    Done,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(rename = "ReportArgs"))]
pub struct OrgReport {
    pub title: String,
    pub body: String,
    pub status: ReportStatus,
    pub blocked_on: Option<String>,
}

impl OrgReport {
    /// The title and the body, an empty one left out; `None` when both are empty. A done keeps it
    /// with its attempt.
    fn summary(&self) -> Option<String> {
        let parts: Vec<&str> = [self.title.trim(), self.body.trim()].into_iter().filter(|s| !s.is_empty()).collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }
}

/// What the report changed. Every report is kept as a command record, including reports that
/// change nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum ReportEffect {
    Recorded,
    /// The same blocking reason or `done` was already recorded.
    Unchanged,
    /// The turn has ended, or belongs to an attempt that is no longer open. A late call never
    /// advances state (data-model.md §3.1).
    Late,
    /// The turn belongs to no task: there is nothing for `done` or a question to advance.
    NoTask,
}

impl Command for OrgReport {
    const NAME: &'static str = "org_report";
    type Output = ReportEffect;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<ReportEffect> {
        let Caller::Role { role, turn_id } = caller else {
            return Err(Error::rejected("forbidden", "org_report comes from a role's turn"));
        };
        let turn = load_turn(cx.tx, turn_id)?;
        if &turn.role != role {
            return Err(Error::rejected("forbidden", format!("turn {} belongs to {}", turn.id, turn.role)));
        }
        if turn.state == TurnState::Ended {
            cx.emit("report.late", &turn.id, json!({ "status": self.status }))?;
            return Ok(ReportEffect::Late);
        }
        let task = match current_task(cx, &turn)? {
            Some(task) => task,
            None if self.status == ReportStatus::Progress => {
                cx.emit("report.progress", &turn.id, json!({ "role": role, "title": self.title }))?;
                return Ok(ReportEffect::Recorded);
            }
            // A blocking question or `done` needs the task in execution in this attempt.
            None if turn.task_id.is_none() => {
                cx.emit("report.no_task", &turn.id, json!({ "status": self.status }))?;
                return Ok(ReportEffect::NoTask);
            }
            None => {
                cx.emit("report.late", &turn.id, json!({ "status": self.status }))?;
                return Ok(ReportEffect::Late);
            }
        };

        let effect = match self.status {
            ReportStatus::Progress => {
                question::withdraw_task(cx, &task.id)?;
                ReportEffect::Recorded
            }
            ReportStatus::Blocked => {
                let reason = self.blocked_on.clone().unwrap_or_else(|| self.title.clone());
                let (_, asked) = question::ask(cx, role, Some(&task.id), &turn.id, &reason)?;
                if asked { ReportEffect::Recorded } else { ReportEffect::Unchanged }
            }
            ReportStatus::Done => {
                if turn.done_at.is_some() {
                    ReportEffect::Unchanged
                } else {
                    // The CLI may still be running. The task moves to acceptance when the turn
                    // ends; the role stays occupied meanwhile.
                    cx.tx.execute("UPDATE turn SET done_at = ?2 WHERE id = ?1", params![turn.id, cx.now])?;
                    cx.tx.execute(
                        "UPDATE attempt SET done_turn_id = ?2, done_summary = ?3 WHERE id = ?1 AND done_turn_id IS NULL",
                        params![turn.attempt_id, turn.id, self.summary()],
                    )?;
                    question::withdraw_task(cx, &task.id)?;
                    ReportEffect::Recorded
                }
            }
        };
        if effect == ReportEffect::Recorded {
            cx.emit(
                "report.recorded",
                &task.id,
                json!({ "turn_id": turn.id, "status": self.status, "title": self.title }),
            )?;
        }
        Ok(effect)
    }
}

/// The task this turn works on, if the task is still executing in the turn's attempt.
fn current_task(cx: &Cx<'_>, turn: &Turn) -> Result<Option<Task>> {
    let (Some(task_id), Some(attempt_id)) = (&turn.task_id, &turn.attempt_id) else {
        return Ok(None);
    };
    let task = load_task(cx.tx, task_id)?;
    if task.phase != Phase::Executing {
        return Ok(None);
    }
    let open = open_attempt(cx.tx, task_id)?;
    Ok(open.filter(|a| &a.id == attempt_id).map(|_| task))
}

/// A role says the manager's interpretation differs from the user's words (manager-actions.md
/// §2.3). It goes to the user unfiltered, because it questions the manager herself; the runtime
/// puts it in the user's conversation with her, and she gets it too, to correct herself.
#[derive(Debug, Serialize, Deserialize)]
pub struct FlagMismatch {
    /// What differs, in the role's own words.
    pub body: String,
}

impl Command for FlagMismatch {
    const NAME: &'static str = "flag_mismatch";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        let Caller::Role { role, turn_id } = caller else {
            return Err(Error::rejected("forbidden", "flag_mismatch comes from a role's turn"));
        };
        let turn = load_turn(cx.tx, turn_id)?;
        if &turn.role != role || turn.state == TurnState::Ended {
            return Err(Error::rejected("turn_ended", format!("turn {} has ended", turn.id)));
        }
        if self.body.trim().is_empty() {
            return Err(Error::rejected("missing_field", "say what differs"));
        }
        cx.emit("mismatch.flagged", &turn.id, json!({ "role": role, "task_id": turn.task_id, "body": self.body }))?;
        Ok(())
    }
}
