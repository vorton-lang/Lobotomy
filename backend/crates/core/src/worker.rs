//! Tasks a worker runs (tech-plan.md §7 step 1, group 7; worker-interface.md): what the
//! orchestrator knows of each, kept from the worker's events. The task's phase follows the worker:
//! a delivery waits for acceptance; a closed task is done or abandoned; everything else executes,
//! a failed or stopped attempt included, until someone closes the task or revises it.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::task::{Phase, bump_revision, close_task, load_task, record_decision};
use lobotomy_worker_protocol::event::{
    Artifact, AttemptState, Blocked, Event, EventBody, Evidence, Question, TaskSnapshot,
};
use lobotomy_worker_protocol::ops::CloseOutcome;

/// A worker task's current attempt, as the worker last said.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct WorkerStatus {
    pub state: AttemptState,
    pub blocked: Option<Blocked>,
    pub reason: Option<String>,
    pub question: Option<Question>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct WorkerTask {
    pub worker: String,
    pub spec_version: i64,
    pub attempt_id: String,
    pub status: WorkerStatus,
    /// The current attempt's artifacts, at their current versions.
    pub artifacts: Vec<Artifact>,
    /// The evidence on those versions.
    pub evidence: Vec<Evidence>,
    /// The worker's last event applied.
    pub seq: i64,
}

impl WorkerTask {
    /// The current attempt's change, if it delivered one.
    pub fn change(&self) -> Option<&Artifact> {
        self.artifacts.iter().find(|a| a.kind == lobotomy_worker_protocol::event::ArtifactKind::Change)
    }
}

fn status_json(status: &WorkerStatus) -> Result<String> {
    Ok(serde_json::to_string(status)?)
}

pub fn worker_task(conn: &Connection, task_id: &str) -> Result<Option<WorkerTask>> {
    let row: Option<(String, i64, String, String, i64)> = conn
        .query_row(
            "SELECT worker, spec_version, attempt_id, status, seq FROM worker_task WHERE task_id = ?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((worker, spec_version, attempt_id, status, seq)) = row else { return Ok(None) };
    let mut stmt = conn.prepare("SELECT current FROM worker_artifact WHERE task_id = ?1 AND attempt_id = ?2")?;
    let artifacts: Vec<Artifact> = stmt
        .query_map([task_id, &attempt_id], |r| r.get::<_, String>(0))?
        .map(|text| Ok(serde_json::from_str(&text?)?))
        .collect::<Result<_>>()?;
    let mut evidence = Vec::new();
    for artifact in &artifacts {
        let mut stmt = conn.prepare(
            "SELECT current FROM worker_evidence WHERE task_id = ?1 AND artifact_id = ?2 AND version = ?3 ORDER BY name",
        )?;
        let rows = stmt.query_map([task_id, &artifact.artifact_id, &artifact.version], |r| r.get::<_, String>(0))?;
        for text in rows {
            evidence.push(serde_json::from_str(&text?)?);
        }
    }
    Ok(Some(WorkerTask {
        worker,
        spec_version,
        attempt_id,
        status: serde_json::from_str(&status)?,
        artifacts,
        evidence,
        seq,
    }))
}

/// The worker's last event applied for each of its tasks: where its subscription resumes.
pub fn worker_seqs(conn: &Connection, worker: &str) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare("SELECT task_id, seq FROM worker_task WHERE worker = ?1")?;
    let rows = stmt.query_map([worker], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Whether an open worker task is on its way: its attempt not yet delivered, stopped, failed or
/// blocked. Such a project has not settled (manager-actions.md §6).
pub fn moving(conn: &Connection) -> Result<bool> {
    let mut stmt = conn.prepare(
        "SELECT w.status FROM worker_task w JOIN task t ON t.id = w.task_id WHERE t.phase IN ('queued', 'executing')",
    )?;
    let statuses = stmt.query_map([], |r| r.get::<_, String>(0))?;
    for status in statuses {
        let status: WorkerStatus = serde_json::from_str(&status?)?;
        if matches!(status.state, AttemptState::Submitted | AttemptState::Preparing | AttemptState::Working) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A state the orchestrator should look at: Angela wakes for it (tech-plan.md §7 step 1).
pub fn wants_attention(state: AttemptState) -> bool {
    matches!(
        state,
        AttemptState::Delivered
            | AttemptState::Blocked
            | AttemptState::Failed
            | AttemptState::Unknown
            | AttemptState::Canceled
    )
}

/// The task's phase, as the worker's state moves it: a delivery waits for acceptance, and review
/// rework goes back to executing. Closing is the orchestrator's (`CloseWorkerTask`).
fn sync_phase(cx: &mut Cx<'_>, task_id: &str, state: AttemptState) -> Result<()> {
    let task = load_task(cx.tx, task_id)?;
    if task.phase.is_closed() {
        return Ok(());
    }
    let phase = match state {
        AttemptState::Delivered => Phase::Accepting,
        // Merged, waiting for the orchestrator to close it.
        AttemptState::Adopted => return Ok(()),
        _ => Phase::Executing,
    };
    if task.phase != phase {
        cx.tx
            .execute("UPDATE task SET phase = ?2, queue_pos = NULL WHERE id = ?1", params![task_id, phase.as_str()])?;
        bump_revision(cx, task_id)?;
        if phase == Phase::Accepting {
            cx.emit("task.accepting", task_id, json!({}))?;
        }
    }
    Ok(())
}

fn emit_state(cx: &mut Cx<'_>, task_id: &str, status: &WorkerStatus) -> Result<()> {
    let need = status.blocked.as_ref().map(|b| b.need);
    let payload = json!({ "state": status.state, "need": need, "reason": status.reason });
    cx.emit("worker.state_changed", task_id, payload)?;
    Ok(())
}

fn upsert_artifact(cx: &Cx<'_>, task_id: &str, attempt_id: &str, artifact: &Artifact) -> Result<()> {
    cx.tx.execute(
        "INSERT INTO worker_artifact (task_id, artifact_id, attempt_id, current) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (task_id, artifact_id) DO UPDATE SET current = excluded.current",
        params![task_id, artifact.artifact_id, attempt_id, serde_json::to_string(artifact)?],
    )?;
    Ok(())
}

fn upsert_evidence(cx: &Cx<'_>, task_id: &str, evidence: &Evidence) -> Result<()> {
    cx.tx.execute(
        "INSERT INTO worker_evidence (task_id, artifact_id, version, name, current) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (task_id, artifact_id, version, name) DO UPDATE SET current = excluded.current",
        params![task_id, evidence.artifact_id, evidence.version, evidence.name, serde_json::to_string(evidence)?],
    )?;
    Ok(())
}

fn require_worker_task(cx: &Cx<'_>, task_id: &str) -> Result<WorkerTask> {
    worker_task(cx.tx, task_id)?
        .ok_or_else(|| Error::rejected("not_a_worker_task", format!("no worker runs {task_id}")))
}

/// The worker took a queued task, as attempt `attempt_id`; from now on its events say how it goes.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerTaskCreated {
    pub task_id: String,
    pub worker: String,
    pub spec_version: i64,
    pub attempt_id: String,
}

impl Command for WorkerTaskCreated {
    const NAME: &'static str = "worker_task_created";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let task = load_task(cx.tx, &self.task_id)?;
        if task.phase != Phase::Queued {
            return Err(Error::rejected("not_queued", format!("task {} is {}", task.id, task.phase.as_str())));
        }
        let status = WorkerStatus { state: AttemptState::Submitted, blocked: None, reason: None, question: None };
        cx.tx.execute(
            "INSERT INTO worker_task (task_id, worker, spec_version, attempt_id, status) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![self.task_id, self.worker, self.spec_version, self.attempt_id, status_json(&status)?],
        )?;
        cx.tx.execute("UPDATE task SET phase = 'executing', queue_pos = NULL WHERE id = ?1", [&self.task_id])?;
        bump_revision(cx, &self.task_id)?;
        cx.emit("worker.task_created", &self.task_id, json!({ "worker": self.worker, "attempt_id": self.attempt_id }))?;
        Ok(())
    }
}

/// A new spec version started attempt `attempt_id` (worker-interface.md §3 `revise`).
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerTaskRevised {
    pub task_id: String,
    pub spec_version: i64,
    pub attempt_id: String,
}

impl Command for WorkerTaskRevised {
    const NAME: &'static str = "worker_task_revised";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let current = require_worker_task(cx, &self.task_id)?;
        if current.attempt_id == self.attempt_id {
            return Ok(());
        }
        let status = WorkerStatus { state: AttemptState::Submitted, blocked: None, reason: None, question: None };
        cx.tx.execute(
            "UPDATE worker_task SET spec_version = ?2, attempt_id = ?3, status = ?4 WHERE task_id = ?1",
            params![self.task_id, self.spec_version, self.attempt_id, status_json(&status)?],
        )?;
        sync_phase(cx, &self.task_id, AttemptState::Submitted)?;
        cx.emit("worker.revised", &self.task_id, json!({ "spec_version": self.spec_version }))?;
        Ok(())
    }
}

/// One of the worker's events, applied once and in order: an event at or before the last applied
/// changes nothing. Returns whether it applied.
#[derive(Debug, Serialize, Deserialize)]
pub struct ApplyWorkerEvent {
    pub task_id: String,
    pub event: Event,
}

impl Command for ApplyWorkerEvent {
    const NAME: &'static str = "apply_worker_event";
    type Output = bool;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<bool> {
        caller.require_runtime()?;
        let mut current = require_worker_task(cx, &self.task_id)?;
        let event = &self.event;
        if event.seq as i64 <= current.seq {
            return Ok(false);
        }
        let task_id = self.task_id.as_str();
        // An earlier attempt's events are recorded by their seq only (worker-interface.md §9).
        let ours = event.attempt_id == current.attempt_id;
        match &event.body {
            EventBody::StateChanged { state, blocked, reason } if ours => {
                current.status.state = *state;
                current.status.blocked = blocked.clone();
                current.status.reason = reason.clone();
                emit_state(cx, task_id, &current.status)?;
                sync_phase(cx, task_id, *state)?;
            }
            EventBody::QuestionOpened { question } if ours => {
                current.status.question = Some(question.clone());
                cx.emit("worker.question", task_id, json!({ "body": question.body }))?;
            }
            EventBody::QuestionClosed { .. } if ours => current.status.question = None,
            EventBody::ArtifactPublished { artifact } => {
                upsert_artifact(cx, task_id, &event.attempt_id, artifact)?;
                let payload = json!({
                    "artifact_id": artifact.artifact_id, "version": artifact.version, "kind": artifact.kind,
                    "url": artifact.url, "title": artifact.title,
                });
                cx.emit("worker.artifact", task_id, payload)?;
            }
            EventBody::EvidenceUpdated { evidence } => {
                upsert_evidence(cx, task_id, evidence)?;
                let payload = json!({ "name": evidence.name, "status": evidence.status, "version": evidence.version });
                cx.emit("worker.evidence", task_id, payload)?;
            }
            // Questioning the manager goes to the user as it is (manager-actions.md §2.3).
            EventBody::MismatchFlagged { body } => {
                let payload = json!({ "role": current.worker, "task_id": task_id, "body": body });
                cx.emit("mismatch.flagged", task_id, payload)?;
            }
            _ => {}
        }
        cx.tx.execute(
            "UPDATE worker_task SET status = ?2, seq = ?3 WHERE task_id = ?1",
            params![task_id, status_json(&current.status)?, event.seq as i64],
        )?;
        Ok(true)
    }
}

/// The worker's snapshot of a task after (re)subscribing: it is authoritative
/// (worker-interface.md §9), unless an event after it already applied. It does not move the seq:
/// the events it covers that the worker resends after it still apply, in order, and end where it
/// stands. A task this project does not know is left alone.
#[derive(Debug, Serialize, Deserialize)]
pub struct ApplyWorkerSnapshot {
    pub snapshot: TaskSnapshot,
}

impl Command for ApplyWorkerSnapshot {
    const NAME: &'static str = "apply_worker_snapshot";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let s = &self.snapshot;
        let Some(current) = worker_task(cx.tx, &s.task_id)? else { return Ok(()) };
        if current.seq > s.seq as i64 {
            return Ok(());
        }
        let status = WorkerStatus {
            state: s.state,
            blocked: s.blocked.clone(),
            reason: s.reason.clone(),
            question: s.question.clone(),
        };
        cx.tx.execute(
            "UPDATE worker_task SET spec_version = ?2, attempt_id = ?3, status = ?4 WHERE task_id = ?1",
            params![s.task_id, s.spec_version, s.attempt_id, status_json(&status)?],
        )?;
        for artifact in &s.artifacts {
            upsert_artifact(cx, &s.task_id, &s.attempt_id, artifact)?;
        }
        for evidence in &s.evidence {
            upsert_evidence(cx, &s.task_id, evidence)?;
        }
        if current.status != status || current.attempt_id != s.attempt_id {
            emit_state(cx, &s.task_id, &status)?;
        }
        sync_phase(cx, &s.task_id, s.state)?;
        // Closed there but not here: the close went out and its record did not land.
        let task = load_task(cx.tx, &s.task_id)?;
        if let (Some(outcome), false) = (s.closed, task.phase.is_closed()) {
            let phase = if outcome == CloseOutcome::Adopted { Phase::Done } else { Phase::Abandoned };
            close_task(cx, &task, phase)?;
        }
        Ok(())
    }
}

/// The orchestrator closed a task a worker ran, adopted or abandoned: the decision is recorded and
/// the task closes (data-model.md §4.2).
#[derive(Debug, Serialize, Deserialize)]
pub struct CloseWorkerTask {
    pub request_id: String,
    pub task_id: String,
    pub outcome: CloseOutcome,
    pub reason: String,
}

impl Command for CloseWorkerTask {
    const NAME: &'static str = "close_worker_task";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user_or_manager()?;
        let worker = require_worker_task(cx, &self.task_id)?;
        let task = load_task(cx.tx, &self.task_id)?;
        if task.phase.is_closed() {
            return Err(Error::rejected("task_closed", format!("task {} is {}", task.id, task.phase.as_str())));
        }
        let (kind, phase, event) = match self.outcome {
            CloseOutcome::Adopted => ("accept", Phase::Done, "task.done"),
            CloseOutcome::Abandoned => ("abandon", Phase::Abandoned, "task.abandoned"),
        };
        let version = worker.change().map(|c| c.version.clone());
        let detail = json!({ "reason": self.reason, "attempt_id": worker.attempt_id, "version": version });
        let decision = record_decision(cx, kind, Some(&task.id), caller, detail)?;
        close_task(cx, &task, phase)?;
        cx.emit(event, &task.id, json!({ "decision_id": decision, "reason": self.reason }))?;
        Ok(())
    }
}

/// The message went to the task's worker (worker-interface.md §3 `send_message`): it no longer
/// waits in the role's inbox, and stays as the record of what was said.
#[derive(Debug, Serialize, Deserialize)]
pub struct MessageForwarded {
    pub message_id: String,
}

impl Command for MessageForwarded {
    const NAME: &'static str = "message_forwarded";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let changed = cx.tx.execute(
            "UPDATE message SET state = 'bound', delivered_at = ?2 WHERE id = ?1 AND state = 'queued'",
            params![self.message_id, cx.now],
        )?;
        if changed > 0 {
            cx.emit("message.forwarded", &self.message_id, json!({}))?;
        }
        Ok(())
    }
}
