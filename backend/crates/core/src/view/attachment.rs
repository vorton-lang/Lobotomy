//! What `[[att:<id>]]` in the manager's reply shows (manager-actions.md §2.1): the attachment's
//! source, the version it is bound to, and whether it still holds now. The runtime writes the line
//! from the record the id names; the manager only names the id and cannot write the line.
//!
//! The record itself does not change after the manager cited it: tasks aside, these records are
//! never rewritten. What changes is whether it still holds, read at each rendering.

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use crate::error::Result;
use crate::report::{OrgReport, ReportEffect};
use crate::task::{Phase, load_task};

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum Attachment {
    Task {
        task_id: String,
        title: String,
        executor: String,
        phase: Phase,
        paused: bool,
    },
    /// An executor's report, with the title it wrote itself.
    Report {
        task_id: Option<String>,
        title: Option<String>,
        role: String,
        report_title: String,
        status: String,
        at: i64,
        /// The length of its body, in characters.
        chars: usize,
        /// `late` and `no_task` changed nothing (data-model.md §9.2).
        effect: Option<ReportEffect>,
    },
}

/// The attachment an id names in this project, or `None` when nothing here has that id or the
/// record is not one the manager can attach.
pub fn attachment(conn: &Connection, id: &str) -> Result<Option<Attachment>> {
    let prefix = id.split('_').next().unwrap_or_default();
    match prefix {
        "task" => task(conn, id),
        "cmd" => report(conn, id),
        _ => Ok(None),
    }
}

fn task(conn: &Connection, id: &str) -> Result<Option<Attachment>> {
    let Some(task) = load_task(conn, id).ok() else {
        return Ok(None);
    };
    Ok(Some(Attachment::Task {
        task_id: task.id,
        title: task.title,
        executor: task.executor,
        phase: task.phase,
        paused: task.paused,
    }))
}

fn title(conn: &Connection, task_id: &str) -> Result<String> {
    Ok(conn.query_row("SELECT title FROM task WHERE id = ?1", [task_id], |r| r.get(0))?)
}

fn report(conn: &Connection, id: &str) -> Result<Option<Attachment>> {
    let row: Option<(String, String, i64, String, Option<String>)> = conn
        .query_row(
            "SELECT c.args, c.result, c.created_at, t.role, t.task_id FROM command_record c JOIN turn t ON t.id = c.turn_id
             WHERE c.id = ?1 AND c.name = 'org_report'",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((args, result, at, role, task_id)) = row else {
        return Ok(None);
    };
    let report: OrgReport = serde_json::from_str(&args)?;
    let status = serde_json::to_value(report.status)?.as_str().unwrap_or_default().to_owned();
    Ok(Some(Attachment::Report {
        title: task_id.as_deref().map(|t| title(conn, t)).transpose()?,
        task_id,
        role,
        report_title: report.title,
        status,
        at,
        chars: report.body.chars().count(),
        effect: serde_json::from_str(&result).ok(),
    }))
}
