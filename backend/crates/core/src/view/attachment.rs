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
use crate::task::{Phase, list_attempts, load_task};
use crate::verify::{VerificationState, evidence_is_stale, latest_verification, load_verification};

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    Task {
        task_id: String,
        title: String,
        executor: String,
        phase: Phase,
        paused: bool,
    },
    /// An attempt's candidate.
    Candidate {
        task_id: String,
        title: String,
        /// The attempt that produced it.
        seq: i64,
        commit: Option<String>,
        at: i64,
        /// A newer attempt's candidate replaced it.
        superseded: bool,
    },
    Verification {
        task_id: String,
        title: String,
        /// The attempt whose candidate it verified.
        seq: i64,
        state: VerificationState,
        /// The integration version the candidate was rebased onto.
        base: String,
        config_version: i64,
        at: i64,
        /// A newer candidate or a newer verification replaced it.
        superseded: bool,
        /// The integration version or the checks changed since it ran (data-model.md §4.5).
        stale: bool,
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
        "cap" => candidate(conn, id),
        "ver" => verification(conn, id),
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

/// The id of the task's latest candidate.
fn latest_candidate(conn: &Connection, task_id: &str) -> Result<Option<String>> {
    Ok(list_attempts(conn, task_id)?.into_iter().rev().find_map(|a| a.candidate_id))
}

fn candidate(conn: &Connection, id: &str) -> Result<Option<Attachment>> {
    let row: Option<(String, i64, Option<String>, i64)> = conn
        .query_row(
            "SELECT a.task_id, a.seq, c.commit_id, c.created_at FROM attempt a JOIN capture c ON c.id = a.candidate_id
             WHERE a.candidate_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((task_id, seq, commit, at)) = row else {
        return Ok(None);
    };
    let superseded = latest_candidate(conn, &task_id)?.as_deref() != Some(id);
    Ok(Some(Attachment::Candidate { title: title(conn, &task_id)?, task_id, seq, commit, at, superseded }))
}

fn verification(conn: &Connection, id: &str) -> Result<Option<Attachment>> {
    let Some(v) = load_verification(conn, id).ok() else {
        return Ok(None);
    };
    let seq: i64 = conn.query_row("SELECT seq FROM attempt WHERE id = ?1", [&v.attempt_id], |r| r.get(0))?;
    let latest = latest_verification(conn, &v.task_id)?.map(|l| l.id);
    let superseded =
        latest.as_deref() != Some(id) || latest_candidate(conn, &v.task_id)?.as_deref() != Some(&v.capture_id);
    Ok(Some(Attachment::Verification {
        title: title(conn, &v.task_id)?,
        stale: evidence_is_stale(conn, &v)?,
        task_id: v.task_id,
        seq,
        state: v.state,
        base: v.base,
        config_version: v.config_version,
        at: v.finished_at.unwrap_or(v.created_at),
        superseded,
    }))
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
