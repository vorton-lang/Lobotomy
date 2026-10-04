//! Verification, acceptance and the preview of the integration version
//! (harness-adapter.md §4.2, §4.3; data-model.md §4, §5).
//!
//! A candidate is rebased onto the current integration version and checked in the fixed
//! verification site. Accepting it publishes the rebased commit: the decision, the compare-and-set
//! of the integration head, closing the task and the preview's outbox record commit together.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::capture::load_capture;
use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::id::new_id;
use crate::item::Blob;
use crate::project::{ProjectConfig, config_version, current_config, require_project};
use crate::task::{Phase, Task, load_task, queue_message, record_decision};
use crate::workspace::{plan, role_slot};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    Running,
    Passed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Verification {
    pub id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub capture_id: String,
    /// The integration version the candidate was rebased onto.
    pub base: String,
    pub config_version: i64,
    /// The rebased commit: the next integration version if the user accepts it.
    pub commit_id: Option<String>,
    pub conflicts: Vec<String>,
    pub state: VerificationState,
}

const SELECT: &str =
    "SELECT id, task_id, attempt_id, capture_id, base, config_version, commit_id, conflicts, state FROM verification";

fn query(conn: &Connection, filter: &str, args: impl rusqlite::Params) -> Result<Vec<Verification>> {
    let mut stmt = conn.prepare(&format!("{SELECT} {filter}"))?;
    let rows = stmt.query_map(args, |r| {
        let v = Verification {
            id: r.get(0)?,
            task_id: r.get(1)?,
            attempt_id: r.get(2)?,
            capture_id: r.get(3)?,
            base: r.get(4)?,
            config_version: r.get(5)?,
            commit_id: r.get(6)?,
            conflicts: vec![],
            state: VerificationState::Running,
        };
        Ok((v, r.get::<_, Option<String>>(7)?, r.get::<_, String>(8)?))
    })?;
    rows.map(|row| {
        let (mut v, conflicts, state) = row?;
        v.conflicts = conflicts.as_deref().map(serde_json::from_str).transpose()?.unwrap_or_default();
        v.state = serde_json::from_value(Value::String(state))?;
        Ok(v)
    })
    .collect()
}

pub fn load_verification(conn: &Connection, id: &str) -> Result<Verification> {
    query(conn, "WHERE id = ?1", [id])?.pop().ok_or_else(|| Error::rejected("not_found", format!("no verification {id}")))
}

pub fn latest_verification(conn: &Connection, task_id: &str) -> Result<Option<Verification>> {
    Ok(query(conn, "WHERE task_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1", [task_id])?.pop())
}

pub fn running_verifications(conn: &Connection) -> Result<Vec<Verification>> {
    query(conn, "WHERE state = 'running' ORDER BY created_at, id", [])
}

/// Tasks waiting in a phase, oldest first.
pub fn tasks_in_phase(conn: &Connection, phase: Phase) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM task WHERE phase = ?1 ORDER BY created_at, id")?;
    let rows = stmt.query_map([phase.as_str()], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The task's latest attempt and the candidate it produced, if any.
fn last_candidate(conn: &Connection, task_id: &str) -> Result<Option<(String, String)>> {
    Ok(conn
        .query_row(
            "SELECT id, candidate_id FROM attempt WHERE task_id = ?1 ORDER BY seq DESC LIMIT 1",
            [task_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()?
        .and_then(|(attempt, candidate)| candidate.map(|c| (attempt, c))))
}

/// The latest candidate the task ever produced, for reopening (data-model.md §4.6).
pub fn latest_candidate_commit(conn: &Connection, task_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT c.commit_id FROM attempt a JOIN capture c ON c.id = a.candidate_id
             WHERE a.task_id = ?1 ORDER BY a.seq DESC LIMIT 1",
            [task_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// Everything the runtime needs to carry out a verification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerificationPlan {
    pub verification_id: String,
    pub candidate: String,
    pub base: String,
    pub branch: String,
    pub config: ProjectConfig,
    /// The commit message of the integration version (harness-adapter.md §4.3).
    pub message: String,
    pub author_name: String,
    pub author_email: String,
}

pub fn verification_plan(conn: &Connection, id: &str) -> Result<VerificationPlan> {
    let v = load_verification(conn, id)?;
    let capture = load_capture(conn, &v.capture_id)?;
    let task = load_task(conn, &v.task_id)?;
    let project = require_project(conn)?;
    let report: Option<String> = conn
        .query_row(
            "SELECT args FROM command_record WHERE turn_id = ?1 AND name = 'org_report'
             AND json_extract(args, '$.status') = 'done' ORDER BY created_at LIMIT 1",
            [&capture.turn_id],
            |r| r.get(0),
        )
        .optional()?;
    let summary = match report.as_deref().map(serde_json::from_str::<Value>).transpose()? {
        Some(args) => {
            let title = args["title"].as_str().unwrap_or_default().trim();
            let body = args["body"].as_str().unwrap_or_default().trim();
            [title, body].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n\n")
        }
        None => String::new(),
    };
    let mut message = task.title.trim().to_owned();
    if !summary.is_empty() {
        message.push_str("\n\n");
        message.push_str(&summary);
    }
    message.push_str(&format!("\n\nLobotomy-Task: {}\nLobotomy-Role: {}\n", task.id, task.executor));
    Ok(VerificationPlan {
        verification_id: v.id,
        candidate: capture.commit_id.ok_or_else(|| Error::rejected("not_pinned", "the candidate is not pinned"))?,
        base: v.base,
        branch: project.branch,
        config: config_version(conn, v.config_version)?,
        message,
        author_name: project.author_name,
        author_email: project.author_email,
    })
}

/// Starts verifying the task's candidate against the current integration version.
#[derive(Debug, Serialize, Deserialize)]
pub struct StartVerification {
    pub task_id: String,
}

impl Command for StartVerification {
    const NAME: &'static str = "start_verification";
    type Output = String;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<String> {
        caller.require_runtime()?;
        let task = load_task(cx.tx, &self.task_id)?;
        if task.phase != Phase::Verifying {
            return Err(Error::rejected("not_verifying", format!("task {} is {}", task.id, task.phase.as_str())));
        }
        if let Some(v) = latest_verification(cx.tx, &task.id)?
            && v.state == VerificationState::Running
        {
            return Err(Error::rejected("verification_running", format!("verification {} is running", v.id)));
        }
        let (attempt_id, capture_id) = last_candidate(cx.tx, &task.id)?
            .ok_or_else(|| Error::rejected("no_candidate", format!("task {} has no candidate", task.id)))?;
        let project = require_project(cx.tx)?;
        let (config_version, _) = current_config(cx.tx)?;
        let id = new_id("ver");
        cx.tx.execute(
            "INSERT INTO verification (id, task_id, attempt_id, capture_id, base, config_version, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7)",
            params![id, task.id, attempt_id, capture_id, project.integration, config_version, cx.now],
        )?;
        cx.emit("verification.started", &id, json!({ "task_id": task.id, "base": project.integration }))?;
        Ok(id)
    }
}

/// The result of one check command.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub command: String,
    pub exit_code: Option<i64>,
    pub timed_out: bool,
    /// `{"text": …}`, with the text moved to the blob store when it is large.
    pub output: Value,
    pub duration_ms: i64,
    /// The last lines of the output, for the message back to the executor.
    pub tail: String,
}

impl CheckOutcome {
    pub fn passed(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FinishVerification {
    pub verification_id: String,
    pub commit: String,
    pub conflicts: Vec<String>,
    pub checks: Vec<CheckOutcome>,
    pub blobs: Vec<Blob>,
}

impl Command for FinishVerification {
    const NAME: &'static str = "finish_verification";
    type Output = VerificationState;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<VerificationState> {
        caller.require_runtime()?;
        let v = load_verification(cx.tx, &self.verification_id)?;
        if v.state != VerificationState::Running {
            return Err(Error::rejected("not_running", format!("verification {} is finished", v.id)));
        }
        crate::item::record_blobs(cx.tx, &self.blobs, cx.now)?;
        for (seq, check) in self.checks.iter().enumerate() {
            cx.tx.execute(
                "INSERT INTO check_run (id, verification_id, seq, command, exit_code, timed_out, output, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    new_id("chk"),
                    v.id,
                    seq as i64,
                    check.command,
                    check.exit_code,
                    check.timed_out,
                    check.output.to_string(),
                    check.duration_ms
                ],
            )?;
        }
        let failed_check = self.checks.iter().find(|c| !c.passed());
        // Checks stop after the first failure, or when the task leaves verification; a
        // verification passes only when every configured check ran and passed.
        let all_ran = self.checks.len() >= config_version(cx.tx, v.config_version)?.checks.len();
        let state = if self.conflicts.is_empty() && failed_check.is_none() && all_ran {
            VerificationState::Passed
        } else {
            VerificationState::Failed
        };
        cx.tx.execute(
            "UPDATE verification SET commit_id = ?2, conflicts = ?3, state = ?4, finished_at = ?5 WHERE id = ?1",
            params![
                v.id,
                self.commit,
                serde_json::to_string(&self.conflicts)?,
                if state == VerificationState::Passed { "passed" } else { "failed" },
                cx.now
            ],
        )?;
        cx.emit("verification.finished", &v.id, json!({ "task_id": v.task_id, "state": state }))?;

        // A verification of an older candidate, or of a task that moved on, advances nothing.
        let task = load_task(cx.tx, &v.task_id)?;
        let current = task.phase == Phase::Verifying
            && last_candidate(cx.tx, &task.id)?.is_some_and(|(_, capture)| capture == v.capture_id);
        if !current {
            return Ok(state);
        }
        if state == VerificationState::Passed {
            cx.tx.execute("UPDATE task SET phase = 'accepting', revision = revision + 1 WHERE id = ?1", [&task.id])?;
            cx.emit("task.accepting", &task.id, json!({ "verification_id": v.id }))?;
            return Ok(state);
        }
        // Failed: the reason goes straight back to the executor, not through the Manager
        // (roles-and-tasks.md §2.2).
        let rebased = rebased_slot(cx.tx, &v, &self.commit)?;
        let body = if !self.conflicts.is_empty() {
            format!(
                "集成版本在你开始这项任务之后有了新的提交，你的候选成果与它冲突。工作目录已更新为合并后的结果，HEAD \
                 指向新的集成版本。以下文件中有冲突标记：\n{}\n\n请解决冲突后再次报告 done。",
                self.conflicts.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n")
            )
        } else {
            let moved = if rebased.is_some() {
                "\n\n集成版本在你开始这项任务之后有了新的提交，工作目录已更新为基于它的结果，HEAD 指向新的集成版本。"
            } else {
                ""
            };
            match failed_check {
                Some(check) => {
                    let why = match (check.timed_out, check.exit_code) {
                        (true, _) => "超时".to_owned(),
                        (false, Some(code)) => format!("退出码为 {code}"),
                        (false, None) => "被终止".to_owned(),
                    };
                    format!(
                        "候选成果没有通过验证：检查命令 `{}` {why}。输出的最后部分：\n```\n{}\n```{moved}\n\n请修复后再次报告 done。",
                        check.command, check.tail
                    )
                }
                None => format!("候选成果没有通过验证：检查命令没有全部运行。{moved}\n\n请再次报告 done。"),
            }
        };
        reenter(cx, &task, rebased, &body)?;
        Ok(state)
    }
}

/// When verification rebased the candidate, the slot becomes the rebased result with HEAD at the
/// new integration version (data-model.md §4.3). Otherwise the slot already holds the candidate.
fn rebased_slot(conn: &Connection, v: &Verification, commit: &str) -> Result<Option<(String, String)>> {
    let capture = load_capture(conn, &v.capture_id)?;
    Ok((capture.base != v.base).then(|| (commit.to_owned(), v.base.clone())))
}

/// Opens the next attempt of a task coming back to execution, with a message for the executor
/// (data-model.md §4.1). The native session carries on; the executor has seen the brief.
fn reenter(cx: &mut Cx<'_>, task: &Task, slot: Option<(String, String)>, body: &str) -> Result<()> {
    let seq: i64 = cx.tx.query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM attempt WHERE task_id = ?1", [&task.id], |r| r.get(0))?;
    let attempt_id = new_id("att");
    cx.tx.execute(
        "INSERT INTO attempt (id, task_id, seq, started_at, code_start) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![attempt_id, task.id, seq, cx.now, slot.as_ref().map(|(target, _)| target)],
    )?;
    cx.tx.execute(
        "UPDATE task SET phase = 'executing', blocked_reason = NULL, revision = revision + 1 WHERE id = ?1",
        [&task.id],
    )?;
    if let Some((target, head)) = &slot
        && let Some(name) = role_slot(cx.tx, &task.executor)?
    {
        plan(cx, &name, target, head, Some(&task.id))?;
    }
    queue_message(cx, &task.executor, &Caller::Runtime, Some(&task.id), body)?;
    cx.emit("attempt.started", &task.id, json!({ "attempt_id": attempt_id, "seq": seq }))?;
    Ok(())
}

/// Evidence is bound to the integration version and the check configuration it ran with
/// (data-model.md §4.5). When either has moved, a task waiting for acceptance is verified again.
#[derive(Debug, Serialize, Deserialize)]
pub struct Reverify {
    pub task_id: String,
}

impl Command for Reverify {
    const NAME: &'static str = "reverify";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let task = load_task(cx.tx, &self.task_id)?;
        if task.phase != Phase::Accepting {
            return Err(Error::rejected("not_accepting", format!("task {} is {}", task.id, task.phase.as_str())));
        }
        let v = latest_verification(cx.tx, &task.id)?
            .ok_or_else(|| Error::rejected("no_verification", format!("task {} was never verified", task.id)))?;
        if !evidence_is_stale(cx.tx, &v)? {
            return Err(Error::rejected("evidence_current", format!("verification {} still holds", v.id)));
        }
        cx.tx.execute("UPDATE task SET phase = 'verifying', revision = revision + 1 WHERE id = ?1", [&task.id])?;
        cx.emit("task.reverifying", &task.id, json!({ "previous": v.id }))?;
        Ok(())
    }
}

pub fn evidence_is_stale(conn: &Connection, v: &Verification) -> Result<bool> {
    let project = require_project(conn)?;
    let (config, _) = current_config(conn)?;
    Ok(v.base != project.integration || v.config_version != config)
}

/// Accepting publishes the candidate (harness-adapter.md §4.2). The request names the
/// verification, the criteria version and the integration head it expects; all must still be
/// current.
#[derive(Debug, Serialize, Deserialize)]
pub struct Accept {
    pub request_id: String,
    pub task_id: String,
    pub verification_id: String,
    pub criteria_version: i64,
    pub expected_integration: String,
}

impl Command for Accept {
    const NAME: &'static str = "accept";
    type Output = String;

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<String> {
        caller.require_user()?;
        let task = load_task(cx.tx, &self.task_id)?;
        if task.phase != Phase::Accepting {
            return Err(Error::rejected("not_accepting", format!("task {} is {}", task.id, task.phase.as_str())));
        }
        let v = latest_verification(cx.tx, &task.id)?
            .filter(|v| v.id == self.verification_id && v.state == VerificationState::Passed)
            .ok_or_else(|| Error::rejected("stale_verification", format!("{} is not the task's passed verification", self.verification_id)))?;
        if task.criteria_version != self.criteria_version {
            return Err(Error::rejected(
                "stale_criteria",
                format!("criteria are at version {}, not {}", task.criteria_version, self.criteria_version),
            ));
        }
        let project = require_project(cx.tx)?;
        // The compare-and-set: the head the user saw, the head the candidate was verified on and
        // the current head must all be the same.
        if project.integration != self.expected_integration || v.base != project.integration {
            return Err(Error::rejected(
                "integration_moved",
                format!("the integration version is {}, verified on {}", project.integration, v.base),
            ));
        }
        if evidence_is_stale(cx.tx, &v)? {
            return Err(Error::rejected("stale_verification", format!("the checks of {} are out of date", v.id)));
        }
        let commit = v.commit_id.clone().ok_or_else(|| Error::rejected("no_commit", "the verification has no commit"))?;
        let detail = json!({
            "verification_id": v.id,
            "capture_id": v.capture_id,
            "criteria_version": self.criteria_version,
            "integration": project.integration,
            "commit": commit,
        });
        let decision = record_decision(cx, "accept", Some(&task.id), caller, detail)?;
        let rev = project.integration_rev + 1;
        cx.tx.execute(
            "UPDATE project SET integration = ?2, integration_rev = ?3 WHERE id = ?1 AND integration = ?4",
            params![project.id, commit, rev, project.integration],
        )?;
        cx.tx.execute(
            "INSERT INTO publication (rev, commit_id, previous, task_id, decision_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![rev, commit, project.integration, task.id, decision, cx.now],
        )?;
        cx.tx.execute("DELETE FROM occupancy WHERE task_id = ?1", [&task.id])?;
        crate::turn::end_task_sessions(cx, &task.id)?;
        cx.tx.execute(
            "UPDATE task SET phase = 'done', closed_at = ?2, revision = revision + 1 WHERE id = ?1",
            params![task.id, cx.now],
        )?;
        cx.tx.execute(
            "INSERT INTO outbox (id, kind, idem_key, payload, state, created_at) VALUES (?1, 'preview', ?2, ?3, 'pending', ?4)",
            params![new_id("obx"), format!("preview:{commit}"), json!({ "target": commit }).to_string(), cx.now],
        )?;
        cx.emit("integration.published", &task.id, json!({ "rev": rev, "commit": commit }))?;
        cx.emit("task.done", &task.id, json!({ "decision_id": decision }))?;
        Ok(commit)
    }
}

/// Sends the candidate back with the user's reason; the next attempt starts (data-model.md §9.2
/// "退回").
#[derive(Debug, Serialize, Deserialize)]
pub struct SendBack {
    pub request_id: String,
    pub task_id: String,
    pub reason: String,
}

impl Command for SendBack {
    const NAME: &'static str = "send_back";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user()?;
        let task = load_task(cx.tx, &self.task_id)?;
        if task.phase != Phase::Accepting {
            return Err(Error::rejected("not_accepting", format!("task {} is {}", task.id, task.phase.as_str())));
        }
        record_decision(cx, "send_back", Some(&task.id), caller, json!({ "reason": self.reason }))?;
        let slot = match latest_verification(cx.tx, &task.id)? {
            Some(v) => match &v.commit_id {
                Some(commit) => rebased_slot(cx.tx, &v, commit)?,
                None => None,
            },
            None => None,
        };
        let body = format!("用户退回了候选成果：\n{}\n\n请修改后再次报告 done。", self.reason);
        reenter(cx, &task, slot, &body)
    }
}

/// The preview to write next: only the latest integration version, skipping older ones
/// (data-model.md §5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PreviewJob {
    pub outbox_id: String,
    pub target: String,
    pub repo_path: String,
    pub branch: String,
    pub previewed: String,
}

pub fn next_preview(conn: &Connection) -> Result<Option<PreviewJob>> {
    let Some(project) = crate::project::load_project(conn)? else {
        return Ok(None);
    };
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT id, payload FROM outbox WHERE kind = 'preview' AND state = 'pending' ORDER BY rowid DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((id, payload)) = row else {
        return Ok(None);
    };
    let payload: Value = serde_json::from_str(&payload)?;
    Ok(Some(PreviewJob {
        outbox_id: id,
        target: payload["target"].as_str().unwrap_or_default().to_owned(),
        repo_path: project.repo_path,
        branch: project.branch,
        previewed: project.previewed,
    }))
}

/// Why the preview stopped, if it did.
pub fn preview_stopped(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT receipt FROM outbox WHERE kind = 'preview' AND state = 'stopped' ORDER BY rowid DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum PreviewResult {
    Written,
    /// The user's repository is not as the last preview left it; nothing was written.
    Stopped { reason: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FinishPreview {
    pub outbox_id: String,
    pub result: PreviewResult,
}

impl Command for FinishPreview {
    const NAME: &'static str = "finish_preview";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let (payload, state): (String, String) = cx
            .tx
            .query_row("SELECT payload, state FROM outbox WHERE id = ?1", [&self.outbox_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(|| Error::rejected("not_found", format!("no outbox record {}", self.outbox_id)))?;
        if state != "pending" {
            return Err(Error::rejected("not_pending", format!("outbox record {} is {state}", self.outbox_id)));
        }
        let target = serde_json::from_str::<Value>(&payload)?["target"].as_str().unwrap_or_default().to_owned();
        match &self.result {
            PreviewResult::Written => {
                cx.tx.execute(
                    "UPDATE outbox SET state = 'done', receipt = ?2, done_at = ?3 WHERE id = ?1",
                    params![self.outbox_id, target, cx.now],
                )?;
                // Previews of older versions are covered by this one. A preview of a newer
                // version, accepted while this one was being written, still has to run (#12).
                cx.tx.execute(
                    "UPDATE outbox SET state = 'superseded', done_at = ?2
                     WHERE kind = 'preview' AND state IN ('pending', 'stopped')
                       AND rowid < (SELECT rowid FROM outbox WHERE id = ?1)",
                    params![self.outbox_id, cx.now],
                )?;
                cx.tx.execute("UPDATE project SET previewed = ?1", [&target])?;
                cx.emit("preview.written", &self.outbox_id, json!({ "target": target }))?;
            }
            PreviewResult::Stopped { reason } => {
                cx.tx.execute("UPDATE outbox SET state = 'stopped', receipt = ?2 WHERE id = ?1", params![self.outbox_id, reason])?;
                // A system failure: the user restores the repository and retries
                // (harness-adapter.md §4.3; manager-actions.md §6).
                cx.emit("preview.stopped", &self.outbox_id, json!({ "target": target, "reason": reason }))?;
            }
        }
        Ok(())
    }
}

/// The user restored the repository; the preview is written again, after the same check
/// (data-model.md §9.2 "重试预览").
#[derive(Debug, Serialize, Deserialize)]
pub struct RetryPreview {
    pub request_id: String,
}

impl Command for RetryPreview {
    const NAME: &'static str = "retry_preview";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user()?;
        let id: String = cx
            .tx
            .query_row(
                "SELECT id FROM outbox WHERE kind = 'preview' AND state = 'stopped' ORDER BY rowid DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| Error::rejected("not_stopped", "the preview has not stopped"))?;
        cx.tx.execute("UPDATE outbox SET state = 'pending', receipt = NULL WHERE id = ?1", [&id])?;
        cx.emit("preview.retry", &id, json!({}))?;
        Ok(())
    }
}
