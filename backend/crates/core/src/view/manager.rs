//! What the manager reads about a project (data-model.md §10.6 "读取带边界").
//!
//! Each read says up to which event of the project it is (`as_of`), and whether each result still
//! holds: a candidate a newer one replaced, or a verification whose integration version or checks
//! changed since it ran, is marked, not left out. What a read covers and finds nothing is empty:
//! "没有". A project the read could not cover is for the caller to say: "不可用".

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use super::{Attention, Overview, last_event};
use crate::capture::{load_capture, unallowed_changes};
use crate::error::Result;
use crate::project::load_project;
use crate::report::{OrgReport, ReportEffect};
use crate::task::{Assignment, AttemptEnd, Phase, Task, Trial, list_attempts, load_task, undelivered_user_messages};
use crate::turn::{Hold, TurnState};
use crate::verify::{VerificationState, evidence_is_stale, task_verifications};

/// How many closed tasks a project read lists, the most recent first.
pub const CLOSED_LISTED: usize = 10;

#[derive(Serialize)]
pub struct ProjectRead {
    /// The last event of the project the read includes.
    pub as_of: i64,
    /// The integration version: what accepting has published so far.
    pub integration: Option<String>,
    pub roles: Vec<RoleLine>,
    /// The open tasks, then the most recently closed.
    pub tasks: Vec<TaskLine>,
    /// Closed tasks left out of `tasks`. Reading a task by its id reads any of them.
    pub closed_not_listed: usize,
    /// What waits for the user in this project ("等你决定").
    pub waiting: Vec<WaitingLine>,
}

#[derive(Serialize)]
pub struct RoleLine {
    pub name: String,
    pub kind: String,
    pub harness: String,
    /// The task that occupies it.
    pub task_id: Option<String>,
    /// A turn of it is registered or running.
    pub running: bool,
    /// Why it waits for the user, if it does: `abnormal_turn`, `capture_stopped` or
    /// `session_unidentified`.
    pub held: Option<&'static str>,
    pub queued_messages: usize,
}

#[derive(Serialize)]
pub struct TaskLine {
    pub id: String,
    pub title: String,
    pub executor: String,
    pub phase: Phase,
    /// Paused; the runtime does not move it on (roles-and-tasks.md §2.2).
    pub paused: bool,
    /// Who paused it: `user` or `manager`. Only the user resumes what the user paused
    /// (roles-and-tasks.md §4).
    pub paused_by: Option<String>,
    /// The executor's question to the user.
    pub blocked_on: Option<String>,
    pub criteria_version: i64,
    pub created_at: i64,
    pub closed_at: Option<i64>,
}

impl TaskLine {
    fn of(task: &Task) -> Self {
        Self {
            id: task.id.clone(),
            title: task.title.clone(),
            executor: task.executor.clone(),
            phase: task.phase,
            paused: task.paused,
            paused_by: task.paused_by.clone(),
            blocked_on: task.blocked_reason.clone(),
            criteria_version: task.criteria_version,
            created_at: task.created_at,
            closed_at: task.closed_at,
        }
    }
}

/// One entry of "等你决定", in a line.
#[derive(Serialize)]
pub struct WaitingLine {
    pub kind: &'static str,
    pub role: Option<String>,
    pub task_id: Option<String>,
    pub what: String,
}

fn waiting_line(attention: &Attention) -> WaitingLine {
    let line = |kind, role: Option<&String>, task_id: Option<&String>, what: String| WaitingLine {
        kind,
        role: role.cloned(),
        task_id: task_id.cloned(),
        what,
    };
    match attention {
        Attention::Hold { role, hold } => line("held", Some(role), None, held(hold).to_owned()),
        Attention::OutsideChanges { role, .. } => line("outside_changes", Some(role), None, "任务之外的改动".into()),
        Attention::UnknownTurn { role, turn_id } => line("unknown_turn", Some(role), None, turn_id.clone()),
        Attention::TaskBlocked { task_id, reason, .. } => line("question", None, Some(task_id), reason.clone()),
        Attention::Stalled { role, task_id, .. } => line("stalled", Some(role), Some(task_id), "没有人推动".into()),
        Attention::Accept { task_id, title, .. } => line("accept", None, Some(task_id), title.clone()),
        Attention::Quota { domain } => line("quota", None, None, domain.harness.clone()),
        Attention::JobFailed { key, reason } => line("job_failed", None, None, format!("{key}：{reason}")),
        Attention::PreviewStopped { reason } => line("preview_stopped", None, None, reason.clone()),
        Attention::Question { question, .. } => line("question_to_user", None, None, question.clone()),
    }
}

fn held(hold: &Hold) -> &'static str {
    match hold {
        Hold::Abnormal { .. } => "abnormal_turn",
        Hold::CaptureStopped { .. } => "capture_stopped",
        Hold::SessionUnidentified { .. } => "session_unidentified",
    }
}

/// A project as the manager reads it, from what the GUI's snapshot reads (`overview`) and what
/// waits for the user (`attention`, which needs the running backend).
pub fn project_read(overview: &Overview, attention: &[Attention]) -> ProjectRead {
    let roles = overview
        .roles
        .iter()
        .map(|r| RoleLine {
            name: r.role.name.clone(),
            kind: r.role.kind.clone(),
            harness: r.role.harness.clone(),
            task_id: r.task_id.clone(),
            running: r.unfinished.as_ref().is_some_and(|t| t.state != TurnState::Unknown),
            held: r.hold.as_ref().map(held),
            queued_messages: r.queued_messages,
        })
        .collect();
    let (closed, open): (Vec<&Task>, Vec<&Task>) =
        overview.tasks.iter().map(|t| &t.task).partition(|t| t.phase.is_closed());
    let mut recent = closed;
    recent.sort_by_key(|t| std::cmp::Reverse((t.closed_at, t.created_at)));
    let closed_not_listed = recent.len().saturating_sub(CLOSED_LISTED);
    recent.truncate(CLOSED_LISTED);
    ProjectRead {
        as_of: overview.seq,
        integration: overview.project.as_ref().map(|p| p.integration.clone()),
        roles,
        tasks: open.into_iter().chain(recent).map(TaskLine::of).collect(),
        closed_not_listed,
        waiting: attention.iter().map(waiting_line).collect(),
    }
}

#[derive(Serialize)]
pub struct TaskRead {
    /// The last event of the project the read includes.
    pub as_of: i64,
    #[serde(flatten)]
    pub line: TaskLine,
    /// The user's words that started it; for a task the manager assigned, the words she quoted.
    pub body: String,
    /// What the manager allowed and when to stop, for a task she assigned.
    pub assignment: Option<Assignment>,
    /// The current completion criteria (data-model.md §4.4).
    pub criteria: String,
    pub attempts: Vec<AttemptRead>,
    /// The executor's reports on it, in order (roles-and-tasks.md §4).
    pub reports: Vec<ReportRead>,
    /// The user's messages to the task that its executor has not got (data-model.md §4.2).
    pub undelivered_messages: usize,
}

#[derive(Serialize)]
pub struct AttemptRead {
    pub seq: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub end_reason: Option<AttemptEnd>,
    /// What the executor's done said.
    pub done_summary: Option<String>,
    pub trial: Option<Trial>,
    pub candidate: Option<CandidateRead>,
}

#[derive(Serialize)]
pub struct CandidateRead {
    pub capture_id: String,
    pub commit_id: Option<String>,
    /// A newer attempt's candidate replaced it.
    pub superseded: bool,
    /// The files it changed although the task allows reading only (roles-and-tasks.md §4).
    pub unallowed_changes: Vec<String>,
    pub verifications: Vec<VerificationRead>,
}

#[derive(Serialize)]
pub struct VerificationRead {
    pub id: String,
    pub state: VerificationState,
    /// The integration version the candidate was rebased onto.
    pub base: String,
    /// The integration version or the checks changed since it ran, so its result no longer
    /// counts (data-model.md §4.5).
    pub stale: bool,
    pub conflicts: Vec<String>,
    pub checks: Vec<CheckLine>,
    pub finished_at: Option<i64>,
}

#[derive(Serialize)]
pub struct CheckLine {
    pub command: String,
    pub exit_code: Option<i64>,
    pub timed_out: bool,
}

#[derive(Serialize)]
pub struct ReportRead {
    /// The report's id, for `[[att:…]]` (manager-actions.md §2.1).
    pub id: String,
    pub at: i64,
    pub role: String,
    #[serde(flatten)]
    pub report: Value,
    /// What it changed: `late` and `no_task` changed nothing (data-model.md §9.2).
    pub effect: Option<ReportEffect>,
}

/// One task as the manager reads it.
pub fn task_read(conn: &Connection, task_id: &str) -> Result<TaskRead> {
    let task = load_task(conn, task_id)?;
    let criteria = crate::task::criteria_text(conn, task_id, task.criteria_version)?;
    let attempts = list_attempts(conn, task_id)?;
    let verifications = task_verifications(conn, task_id)?;
    let has_project = load_project(conn)?.is_some();
    let latest = attempts.iter().rev().find_map(|a| a.candidate_id.clone());
    let mut reads = Vec::new();
    for attempt in &attempts {
        let candidate = match &attempt.candidate_id {
            Some(capture_id) => {
                let capture = load_capture(conn, capture_id)?;
                let mut checks_of = Vec::new();
                for v in verifications.iter().filter(|v| &v.capture_id == capture_id) {
                    checks_of.push(VerificationRead {
                        id: v.id.clone(),
                        state: v.state,
                        base: v.base.clone(),
                        stale: has_project && evidence_is_stale(conn, v)?,
                        conflicts: v.conflicts.clone(),
                        checks: checks(conn, &v.id)?,
                        finished_at: v.finished_at,
                    });
                }
                Some(CandidateRead {
                    unallowed_changes: unallowed_changes(&task, &capture),
                    capture_id: capture.id,
                    commit_id: capture.commit_id,
                    superseded: latest.as_ref() != Some(capture_id),
                    verifications: checks_of,
                })
            }
            None => None,
        };
        reads.push(AttemptRead {
            seq: attempt.seq,
            started_at: attempt.started_at,
            ended_at: attempt.ended_at,
            end_reason: attempt.end_reason,
            done_summary: attempt.done_summary.clone(),
            trial: attempt.trial.clone(),
            candidate,
        });
    }
    Ok(TaskRead {
        as_of: last_event(conn)?,
        line: TaskLine::of(&task),
        body: task.body.clone(),
        assignment: task.assignment.clone(),
        criteria,
        attempts: reads,
        reports: reports(conn, task_id)?,
        undelivered_messages: undelivered_user_messages(conn, task_id)?.len(),
    })
}

fn checks(conn: &Connection, verification_id: &str) -> Result<Vec<CheckLine>> {
    let mut stmt =
        conn.prepare("SELECT command, exit_code, timed_out FROM check_run WHERE verification_id = ?1 ORDER BY seq")?;
    let rows = stmt.query_map([verification_id], |r| {
        Ok(CheckLine { command: r.get(0)?, exit_code: r.get(1)?, timed_out: r.get(2)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn reports(conn: &Connection, task_id: &str) -> Result<Vec<ReportRead>> {
    let mut stmt = conn.prepare(
        "SELECT c.created_at, t.role, c.args, c.result, c.id FROM command_record c JOIN turn t ON t.id = c.turn_id
         WHERE c.name = 'org_report' AND t.task_id = ?1 ORDER BY c.created_at, c.id",
    )?;
    let rows = stmt.query_and_then([task_id], |r| -> Result<ReportRead> {
        let args: String = r.get(2)?;
        let result: String = r.get(3)?;
        // The report as the executor sent it, read back as the command took it.
        let report: OrgReport = serde_json::from_str(&args)?;
        Ok(ReportRead {
            id: r.get(4)?,
            at: r.get(0)?,
            role: r.get(1)?,
            report: serde_json::to_value(report)?,
            effect: serde_json::from_str(&result).ok(),
        })
    })?;
    rows.collect()
}

// ---- what wakes the manager (manager-actions.md §6 "Manager 什么时候运行") ----

/// Whether nothing in the project moves on without the user: no turn registered or running, no
/// capture, verification, slot or preview being written, and nothing the scheduler would start, a
/// queued task or a message to deliver, for a role that does not wait for the user and whose
/// harness is not quota-blocked (`blocked`). A role or task that waits for the user is in "等你决定"
/// instead (manager-actions.md §6 "组织离了用户就无法推进").
pub fn settled(conn: &Connection, blocked: &[String]) -> Result<bool> {
    let any = |sql: &str| -> Result<bool> { Ok(conn.query_row(&format!("SELECT EXISTS ({sql})"), [], |r| r.get(0))?) };
    if any("SELECT 1 FROM turn WHERE state IN ('registered', 'running')")?
        || any("SELECT 1 FROM capture WHERE state = 'intent'")?
        || any("SELECT 1 FROM task WHERE phase = 'verifying'")?
        || any("SELECT 1 FROM workspace WHERE state = 'materializing'")?
        || any("SELECT 1 FROM outbox WHERE state = 'pending'")?
    {
        return Ok(false);
    }
    for role in crate::role::list_roles(conn)? {
        if blocked.contains(&role.harness) || crate::turn::hold(conn, &role.name)?.is_some() {
            continue;
        }
        let occupant = crate::task::occupant(conn, &role.name)?;
        // A free role starts its next queued task.
        if occupant.is_none() && crate::task::next_queued_task(conn, &role.name)?.is_some() {
            return Ok(false);
        }
        // Its messages go in its next turn while it is free or its task executes.
        let deliverable = match &occupant {
            None => true,
            Some(task) => {
                let task = load_task(conn, task)?;
                task.phase == Phase::Executing && !task.paused
            }
        };
        if deliverable && !crate::task::queued_messages(conn, &role.name)?.is_empty() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// How many lines of changes one project gives the manager's input; she reads the rest with
/// `read_events`.
pub const CHANGES_LISTED: usize = 30;

/// What changed in a project since the manager last read it, for her next input: the changes that
/// matter to her, in a line each with the ids she can attach (manager-actions.md §6).
#[derive(Debug, Serialize)]
pub struct ChangesRead {
    /// The events it covers: after `after`, up to `as_of`.
    pub after: i64,
    pub as_of: i64,
    pub lines: Vec<String>,
    /// Lines beyond `CHANGES_LISTED`, left out.
    pub not_listed: usize,
    /// How many of the changes, listed or not, someone other than the manager caused: what she
    /// does not know yet.
    pub by_others: usize,
    /// Roles that said her interpretation differs from the user's words (manager-actions.md §2.3),
    /// a line each; never cut.
    pub mismatches: Vec<String>,
}

pub fn changes_since(conn: &Connection, after: i64) -> Result<ChangesRead> {
    let titles: std::collections::HashMap<String, String> =
        crate::task::list_tasks(conn)?.into_iter().map(|t| (t.id, t.title)).collect();
    let as_of = last_event(conn)?;
    let mut lines = Vec::new();
    let mut mismatches = Vec::new();
    let mut by_others = 0;
    let mut seq = after;
    while seq < as_of {
        let events = super::events_after(conn, seq)?;
        let Some(last) = events.last() else { break };
        seq = last.seq;
        for event in &events {
            if event.kind == "mismatch.flagged" {
                let text = |key: &str| event.payload.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
                let task = event.payload.get("task_id").and_then(Value::as_str).and_then(|t| titles.get(t));
                let of = task.map(|t| format!("（任务「{t}」）")).unwrap_or_default();
                mismatches.push(format!("{}{of}：{}", text("role"), text("body")));
                continue;
            }
            if let Some(line) = change_line(conn, event, &titles)? {
                lines.push(line);
                by_others += usize::from(event.caller.as_deref() != Some("manager"));
            }
        }
    }
    let not_listed = lines.len().saturating_sub(CHANGES_LISTED);
    lines.truncate(CHANGES_LISTED);
    Ok(ChangesRead { after, as_of: seq.max(after), lines, not_listed, by_others, mismatches })
}

/// Who caused a change, as the manager reads it: she is "你".
fn by(caller: Option<&str>) -> &str {
    match caller {
        Some("user") => "用户",
        Some("manager") => "你",
        Some(role) if role.starts_with("role:") => &role[5..],
        _ => "运行时",
    }
}

fn change_line(
    conn: &Connection,
    e: &super::Event,
    titles: &std::collections::HashMap<String, String>,
) -> Result<Option<String>> {
    let task = |id: &str| match titles.get(id) {
        Some(title) => format!("「{title}」（{id}）"),
        None => id.to_owned(),
    };
    let text = |key: &str| e.payload.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    let who = by(e.caller.as_deref());
    let people = matches!(e.caller.as_deref(), Some("user" | "manager"));
    Ok(Some(match e.kind.as_str() {
        "task.created" => format!("{who}建了任务 {}，执行者 {}", task(&e.entity), text("executor")),
        "report.recorded" | "report.progress" => {
            let status = match text("status").as_str() {
                "done" => "完成",
                "blocked" => "提问",
                _ => "进展",
            };
            let of = if e.kind == "report.recorded" { format!("，任务 {}", task(&e.entity)) } else { String::new() };
            let id = e.command_id.as_deref().map(|id| format!("；汇报 {id}")).unwrap_or_default();
            format!("{who} 汇报（{status}）：{}{of}{id}", text("title"))
        }
        "task.blocked_changed" if !text("blocked_reason").is_empty() => {
            format!("任务 {} 的执行者提问：{}", task(&e.entity), text("blocked_reason"))
        }
        "verification.finished" => {
            let state = if text("state") == "passed" { "通过" } else { "未通过" };
            format!("任务 {} 的验证{state}（{}）", task(&text("task_id")), e.entity)
        }
        "task.accepting" => format!("任务 {} 等待验收", task(&e.entity)),
        "task.done" => format!("{who}验收了任务 {}，成果已发布", task(&e.entity)),
        "task.abandoned" => format!("{who}放弃了任务 {}：{}", task(&e.entity), text("reason")),
        "task.reopened" => format!("{who}重开了任务 {}", task(&e.entity)),
        "task.paused_changed" => {
            let what = if e.payload.get("paused").and_then(Value::as_bool) == Some(true) { "暂停" } else { "恢复" };
            format!("{who}{what}了任务 {}", task(&e.entity))
        }
        "task.criteria_changed" => {
            let version = e.payload.get("version").and_then(Value::as_i64).unwrap_or_default();
            format!("{who}把任务 {} 的完成条件改为第 {version} 版", task(&e.entity))
        }
        // A new attempt the user or the manager started: a send-back.
        "attempt.started" if people => {
            let round = e.payload.get("seq").and_then(Value::as_i64).unwrap_or_default();
            format!("{who}退回了任务 {}，开始第 {round} 轮", task(&e.entity))
        }
        // What the user said to a role directly (#23 §3).
        "message.queued" if e.caller.as_deref() == Some("user") => {
            let body: String = conn.query_row("SELECT body FROM message WHERE id = ?1", [&e.entity], |r| r.get(0))?;
            let first = body.lines().next().unwrap_or_default();
            let first: String = first.chars().take(80).collect();
            format!("用户直接给 {} 发了消息：{first}", text("role"))
        }
        "project.config_changed" => format!("{who}修改了项目的检查设置"),
        _ => return Ok(None),
    }))
}

#[derive(Serialize)]
pub struct EventsRead {
    /// The last event of the project when it was read; events after the last one listed are
    /// still to read.
    pub as_of: i64,
    pub events: Vec<super::Event>,
}

/// The project's events after `after`, at most `limit`, each with who caused it (§7.6).
pub fn events_read(conn: &Connection, after: i64, limit: usize) -> Result<EventsRead> {
    let mut events = super::events_after(conn, after)?;
    events.truncate(limit);
    Ok(EventsRead { as_of: last_event(conn)?, events })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_task_beyond_the_listed_ones_is_counted() {
        let task = |id: &str, phase: Phase, closed_at: Option<i64>| super::super::TaskView {
            task: Task {
                id: id.into(),
                title: id.into(),
                body: String::new(),
                executor: "Malkuth".into(),
                phase,
                paused: false,
                paused_by: None,
                blocked_reason: None,
                criteria_version: 1,
                queue_pos: None,
                revision: 1,
                created_at: 0,
                closed_at,
                origin_capture: None,
                assignment: None,
            },
            attempt_seq: None,
            attempt_open: false,
            verification: None,
            undelivered_messages: 0,
            reply_queued: false,
        };
        let mut tasks = vec![task("open", Phase::Executing, None)];
        tasks.extend((0..12).map(|i| task(&format!("done{i}"), Phase::Done, Some(i))));
        let overview = Overview {
            seq: 7,
            project: None,
            config: None,
            roles: Vec::new(),
            tasks,
            preview_stopped: None,
            questions: vec![],
        };
        let read = project_read(&overview, &[]);
        let ids: Vec<_> = read.tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.len(), 1 + CLOSED_LISTED);
        assert_eq!((ids[0], ids[1], ids[CLOSED_LISTED]), ("open", "done11", "done2"));
        assert_eq!((read.as_of, read.closed_not_listed), (7, 2));
    }
}
