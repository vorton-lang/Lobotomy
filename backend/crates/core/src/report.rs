//! `org_report`: how an executor reports progress, a blocking question or `done`
//! (roles-and-tasks.md §4; data-model.md §9.2).

use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::task::{Phase, Task, load_task, open_attempt};
use crate::turn::{Turn, TurnState, load_turn};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Progress,
    Blocked,
    Done,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrgReport {
    pub title: String,
    pub body: String,
    pub status: ReportStatus,
    pub blocked_on: Option<String>,
}

/// What the report changed. Every report is kept as a command record, including reports that
/// change nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportEffect {
    Recorded,
    /// The same blocking reason or `done` was already recorded.
    Unchanged,
    /// The turn has ended, or belongs to an attempt that is no longer open. A late call never
    /// advances state (data-model.md §3.1).
    Late,
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
            None => {
                // A blocking question or `done` needs the task in execution in this attempt.
                cx.emit("report.late", &turn.id, json!({ "status": self.status }))?;
                return Ok(ReportEffect::Late);
            }
        };

        let effect = match self.status {
            ReportStatus::Progress => {
                set_blocked(cx, &task, None)?;
                ReportEffect::Recorded
            }
            ReportStatus::Blocked => {
                let reason = self.blocked_on.clone().unwrap_or_else(|| self.title.clone());
                if task.blocked_reason.as_deref() == Some(reason.as_str()) {
                    ReportEffect::Unchanged
                } else {
                    set_blocked(cx, &task, Some(&reason))?;
                    ReportEffect::Recorded
                }
            }
            ReportStatus::Done => {
                if turn.done_at.is_some() {
                    ReportEffect::Unchanged
                } else {
                    // The CLI may still be running. Capture waits for it to exit; the role stays
                    // occupied meanwhile (harness-adapter.md §4.1).
                    cx.tx.execute("UPDATE turn SET done_at = ?2 WHERE id = ?1", params![turn.id, cx.now])?;
                    cx.tx.execute(
                        "UPDATE attempt SET done_turn_id = ?2 WHERE id = ?1 AND done_turn_id IS NULL",
                        params![turn.attempt_id, turn.id],
                    )?;
                    set_blocked(cx, &task, None)?;
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

fn set_blocked(cx: &mut Cx<'_>, task: &Task, reason: Option<&str>) -> Result<()> {
    if task.blocked_reason.as_deref() == reason {
        return Ok(());
    }
    cx.tx.execute(
        "UPDATE task SET blocked_reason = ?2, revision = revision + 1 WHERE id = ?1",
        params![task.id, reason],
    )?;
    cx.emit("task.blocked_changed", &task.id, json!({ "blocked_reason": reason }))?;
    Ok(())
}
