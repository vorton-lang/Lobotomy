//! Captures: after each turn's CLI exits, the runtime records the slot as an immutable commit
//! (harness-adapter.md §4.1; data-model.md §4.1, §5).
//!
//! The protocol has three steps (#6 §2): the intent is written when the turn ends; the store
//! captures and pins once the writer has stopped; the result is published in a short
//! transaction. A pinned capture of the turn that reported `done` is the attempt's candidate.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::id::new_id;
use crate::project::current_config;
use crate::prompts;
use crate::sql::sql_enum;
use crate::task::{Phase, load_task, open_attempt, record_decision};
use crate::turn::{Outcome, Turn, unfinished_turn};
use crate::workspace::load_workspace;

sql_enum! {
    pub enum CaptureKind {
        /// An ordinary turn's end.
        Turn = "turn",
        /// The capture of the turn that reported `done` (harness-adapter.md §4.1).
        Candidate = "candidate",
        /// The scene a turn left when it did not complete.
        Interrupted = "interrupted",
    }
}

sql_enum! {
    pub enum CaptureState {
        Intent = "intent",
        Pinned = "pinned",
        /// The size guardrail stopped it; nothing was written to the store.
        Oversized = "oversized",
        /// The slot holds content a capture cannot represent; nothing was written.
        Uncovered = "uncovered",
    }
}

/// The user's decision for a capture the runtime stopped (data-model.md §9.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureOptions {
    /// Keep the new files although they exceed the guardrail.
    #[serde(default)]
    pub ignore_guard: bool,
    /// Leave the new files out; changes to tracked files are still captured.
    #[serde(default)]
    pub leave_new_files: bool,
    /// Leave out what cannot be captured: nested repositories, special files, invalid names.
    #[serde(default)]
    pub leave_uncovered: bool,
}

sql_enum! {
    /// What becomes of changes a turn outside any task left in the slot (harness-adapter.md §3, #14).
    pub enum Outside {
        /// The user has not decided; no task starts in the slot until then.
        Pending = "pending",
        /// A task was made from them and starts from them.
        Adopted = "adopted",
        /// The slot was written over; the capture keeps them.
        Discarded = "discarded",
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Capture {
    pub id: String,
    pub turn_id: String,
    pub role: String,
    pub task_id: Option<String>,
    pub attempt_id: Option<String>,
    pub kind: CaptureKind,
    pub workspace_id: String,
    /// The slot's baseline; the capture's commit has it as parent.
    pub base: String,
    pub config_version: i64,
    pub state: CaptureState,
    pub options: CaptureOptions,
    pub commit_id: Option<String>,
    pub detail: Option<Value>,
    pub created_at: i64,
    /// Set for a capture outside any task that changed something.
    pub outside: Option<Outside>,
}

const SELECT: &str = "SELECT id, turn_id, role, task_id, attempt_id, kind, workspace_id, base, config_version, state,
                             options, commit_id, detail, created_at, outside FROM capture";

fn query(conn: &Connection, filter: &str, args: impl rusqlite::Params) -> Result<Vec<Capture>> {
    let mut stmt = conn.prepare(&format!("{SELECT} {filter}"))?;
    let rows = stmt.query_map(args, |r| {
        let capture = Capture {
            id: r.get(0)?,
            turn_id: r.get(1)?,
            role: r.get(2)?,
            task_id: r.get(3)?,
            attempt_id: r.get(4)?,
            kind: r.get(5)?,
            workspace_id: r.get(6)?,
            base: r.get(7)?,
            config_version: r.get(8)?,
            state: r.get(9)?,
            options: CaptureOptions::default(),
            commit_id: r.get(11)?,
            detail: None,
            created_at: r.get(13)?,
            outside: r.get(14)?,
        };
        let json: (String, Option<String>) = (r.get(10)?, r.get(12)?);
        Ok((capture, json))
    })?;
    rows.map(|row| {
        let (mut capture, (options, detail)) = row?;
        capture.options = serde_json::from_str(&options)?;
        capture.detail = detail.as_deref().map(serde_json::from_str).transpose()?;
        Ok(capture)
    })
    .collect()
}

pub fn load_capture(conn: &Connection, id: &str) -> Result<Capture> {
    query(conn, "WHERE id = ?1", [id])?.pop().ok_or_else(|| Error::rejected("not_found", format!("no capture {id}")))
}

/// Captures waiting for the store, oldest first.
pub fn pending_captures(conn: &Connection) -> Result<Vec<Capture>> {
    query(conn, "WHERE state = 'intent' ORDER BY created_at, id", [])
}

/// The role's latest capture: the state of its slot as far as the store knows.
pub fn latest_capture(conn: &Connection, role: &str) -> Result<Option<Capture>> {
    Ok(query(conn, "WHERE role = ?1 ORDER BY created_at DESC, id DESC LIMIT 1", [role])?.pop())
}

/// Records the capture intent for a turn that just ended (harness-adapter.md §4.1 step ①).
pub(crate) fn intend(cx: &mut Cx<'_>, turn: &Turn, outcome: Outcome) -> Result<()> {
    let Some(workspace_id) = &turn.workspace_id else {
        return Ok(());
    };
    let workspace = load_workspace(cx.tx, workspace_id)?;
    let done_turn = match &turn.attempt_id {
        Some(attempt) => cx
            .tx
            .query_row("SELECT done_turn_id FROM attempt WHERE id = ?1 AND ended_at IS NULL", [attempt], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten(),
        None => None,
    };
    let kind = if done_turn.as_deref() == Some(turn.id.as_str()) {
        CaptureKind::Candidate
    } else if outcome != Outcome::Completed {
        CaptureKind::Interrupted
    } else {
        CaptureKind::Turn
    };
    let (config_version, _) = current_config(cx.tx)?;
    let id = new_id("cap");
    cx.tx.execute(
        "INSERT INTO capture (id, turn_id, role, task_id, attempt_id, kind, workspace_id, base, config_version, state,
                              created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'intent', ?10)",
        params![
            id,
            turn.id,
            turn.role,
            turn.task_id,
            turn.attempt_id,
            kind.as_str(),
            workspace.id,
            workspace.head,
            config_version,
            cx.now
        ],
    )?;
    cx.emit("capture.intent", &id, json!({ "turn_id": turn.id, "role": turn.role, "kind": kind }))?;
    Ok(())
}

/// The slot may go to other work only once its content is captured (harness-adapter.md §3): no
/// capture is pending and the latest one was pinned. A capture the runtime stopped keeps the slot
/// until the cause is gone or the user decides (harness-adapter.md §4.1).
pub fn require_captured(conn: &Connection, role: &str) -> Result<()> {
    match latest_capture(conn, role)? {
        Some(c) if c.state == CaptureState::Intent => {
            Err(Error::rejected("capture_pending", format!("capture {} of {role} is pending", c.id)))
        }
        Some(c) if c.state != CaptureState::Pinned => {
            Err(Error::rejected("slot_uncaptured", format!("the latest capture {} of {role} was stopped", c.id)))
        }
        _ => Ok(()),
    }
}

/// The role's latest capture while the runtime has yet to write it.
pub fn capture_in_progress(conn: &Connection, role: &str) -> Result<Option<Capture>> {
    Ok(latest_capture(conn, role)?.filter(|c| c.state == CaptureState::Intent))
}

pub fn require_no_pending_capture(conn: &Connection, role: &str) -> Result<()> {
    match capture_in_progress(conn, role)? {
        Some(c) => Err(Error::rejected("capture_pending", format!("capture {} of {role} is pending", c.id))),
        None => Ok(()),
    }
}

/// Changes a turn outside any task left in the role's slot, while the user has not decided about
/// them: no task starts in the slot until then (harness-adapter.md §3, #14). A later turn outside
/// a task captures the slot again, changes included, so only the latest capture counts.
pub fn outside_changes(conn: &Connection, role: &str) -> Result<Option<Capture>> {
    Ok(latest_capture(conn, role)?.filter(|c| c.outside == Some(Outside::Pending)))
}

/// Settles the pending changes of `capture_id`, which must be the role's latest capture, with no
/// turn running that could change the slot further.
pub(crate) fn settle_outside(cx: &mut Cx<'_>, capture_id: &str, outcome: Outside) -> Result<Capture> {
    let capture = load_capture(cx.tx, capture_id)?;
    let pending = outside_changes(cx.tx, &capture.role)?;
    if pending.as_ref().map(|c| c.id.as_str()) != Some(capture.id.as_str()) {
        return Err(Error::rejected("not_pending", format!("capture {} holds no undecided changes", capture.id)));
    }
    crate::workspace::require_slot_free(cx.tx, &capture.role, Some(&capture.id))?;
    cx.tx.execute("UPDATE capture SET outside = ?2 WHERE id = ?1", params![capture.id, outcome])?;
    cx.emit("capture.outside", &capture.id, json!({ "role": capture.role, "outside": outcome }))?;
    Ok(capture)
}

/// "丢弃任务之外的改动": the slot is written over with the integration version; the capture
/// keeps the changes (#14).
#[derive(Debug, Serialize, Deserialize)]
pub struct DiscardOutsideChanges {
    pub request_id: String,
    pub capture_id: String,
}

impl Command for DiscardOutsideChanges {
    const NAME: &'static str = "discard_outside_changes";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user()?;
        let capture = settle_outside(cx, &self.capture_id, Outside::Discarded)?;
        record_decision(cx, "discard_outside_changes", None, caller, json!({ "capture_id": capture.id }))?;
        let integration = crate::project::require_project(cx.tx)?.integration;
        if let Some(slot) = crate::workspace::role_slot(cx.tx, &capture.role)? {
            crate::workspace::plan(cx, &slot, &integration, &integration, None)?;
        }
        Ok(())
    }
}

/// The role's latest capture, if the runtime stopped it. The role then waits for the user, like
/// after a turn that did not end normally (harness-adapter.md §4.1).
pub fn stopped_capture(conn: &Connection, role: &str) -> Result<Option<Capture>> {
    Ok(latest_capture(conn, role)?.filter(|c| matches!(c.state, CaptureState::Oversized | CaptureState::Uncovered)))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewFile {
    pub path: String,
    pub size: u64,
}

/// Content the capture cannot represent: a nested repository, a special file or an invalid name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UncoveredPath {
    pub kind: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CaptureResult {
    Pinned {
        commit: String,
        /// For a capture outside any task: the paths that differ from its base.
        #[serde(default)]
        changed: Vec<String>,
    },
    Oversized {
        files: Vec<NewFile>,
        total_bytes: u64,
    },
    Uncovered {
        paths: Vec<UncoveredPath>,
    },
}

/// Publishes what the store did with a capture (step ③). A pinned candidate ends its attempt and
/// moves the task to verification. A stopped capture stops the role until the user decides
/// (harness-adapter.md §4.1).
#[derive(Debug, Serialize, Deserialize)]
pub struct FinishCapture {
    pub capture_id: String,
    pub result: CaptureResult,
}

impl Command for FinishCapture {
    const NAME: &'static str = "finish_capture";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let capture = load_capture(cx.tx, &self.capture_id)?;
        if capture.state != CaptureState::Intent {
            return Err(Error::rejected("not_pending", format!("capture {} is finished", capture.id)));
        }
        let (state, commit, detail, outside) = match &self.result {
            // Changes outside any task are kept from being written over: they wait for the user.
            CaptureResult::Pinned { commit, changed } if capture.task_id.is_none() && !changed.is_empty() => (
                CaptureState::Pinned,
                Some(commit.as_str()),
                Some(json!({ "changed": changed })),
                Some(Outside::Pending),
            ),
            CaptureResult::Pinned { commit, .. } => (CaptureState::Pinned, Some(commit.as_str()), None, None),
            CaptureResult::Oversized { files, total_bytes } => {
                (CaptureState::Oversized, None, Some(json!({ "files": files, "total_bytes": total_bytes })), None)
            }
            CaptureResult::Uncovered { paths } => {
                (CaptureState::Uncovered, None, Some(json!({ "paths": paths })), None)
            }
        };
        cx.tx.execute(
            "UPDATE capture SET state = ?2, commit_id = ?3, detail = ?4, finished_at = ?5, outside = ?6 WHERE id = ?1",
            params![capture.id, state, commit, detail.as_ref().map(Value::to_string), cx.now, outside],
        )?;
        cx.emit(
            "capture.finished",
            &capture.id,
            json!({ "role": capture.role, "state": state, "kind": capture.kind, "outside": outside }),
        )?;

        if capture.kind != CaptureKind::Candidate || !matches!(self.result, CaptureResult::Pinned { .. }) {
            return Ok(());
        }
        // Only the candidate of the attempt that is still executing moves anything (data-model.md
        // §3.1: late results keep their source but advance nothing).
        let (Some(task_id), Some(attempt_id)) = (&capture.task_id, &capture.attempt_id) else {
            return Ok(());
        };
        let task = load_task(cx.tx, task_id)?;
        let attempt = open_attempt(cx.tx, task_id)?;
        let current = task.phase == Phase::Executing
            && attempt
                .as_ref()
                .is_some_and(|a| &a.id == attempt_id && a.done_turn_id.as_deref() == Some(&capture.turn_id));
        if !current {
            return Ok(());
        }
        cx.tx.execute(
            "UPDATE attempt SET ended_at = ?2, end_reason = 'candidate', candidate_id = ?3 WHERE id = ?1",
            params![attempt_id, cx.now, capture.id],
        )?;
        cx.tx.execute("UPDATE task SET phase = 'verifying', revision = revision + 1 WHERE id = ?1", [&task.id])?;
        cx.emit("task.candidate", &task.id, json!({ "attempt_id": attempt_id, "capture_id": capture.id }))?;
        Ok(())
    }
}

/// What the executor is told when the user continues past a stopped capture.
pub fn stopped_note(capture: &Capture) -> String {
    let detail = capture.detail.clone().unwrap_or_default();
    let body = match capture.state {
        CaptureState::Oversized => {
            let files: Vec<NewFile> = serde_json::from_value(detail["files"].clone()).unwrap_or_default();
            prompts::capture_oversized(&files, &detail["total_bytes"])
        }
        _ => {
            let paths: Vec<UncoveredPath> = serde_json::from_value(detail["paths"].clone()).unwrap_or_default();
            prompts::capture_uncovered(&paths)
        }
    };
    if capture.kind == CaptureKind::Candidate { prompts::done_not_taken(&body) } else { body }
}

/// The user's way past a stopped capture by deciding what it holds (data-model.md §9.2): the
/// runtime captures the slot again, keeping the new files, or leaving out what was stopped. Only
/// the role's latest capture qualifies, with no turn running, so the slot is still what was
/// captured. A stopped candidate captured this way becomes the candidate.
fn retry(cx: &mut Cx<'_>, caller: &Caller, capture_id: &str, keep: bool) -> Result<()> {
    caller.require_user()?;
    let capture = load_capture(cx.tx, capture_id)?;
    // A decision made earlier for the same capture still holds.
    let mut options = capture.options;
    match (capture.state, keep) {
        (CaptureState::Oversized, true) => options.ignore_guard = true,
        (CaptureState::Oversized, false) => options.leave_new_files = true,
        (CaptureState::Uncovered, false) => options.leave_uncovered = true,
        _ => {
            return Err(Error::rejected(
                "bad_capture_state",
                format!("capture {} was not stopped that way", capture.id),
            ));
        }
    }
    let latest = latest_capture(cx.tx, &capture.role)?;
    if latest.as_ref().map(|c| c.id.as_str()) != Some(capture.id.as_str()) {
        return Err(Error::rejected("not_latest", format!("{} has captured again since {}", capture.role, capture.id)));
    }
    if let Some(turn) = unfinished_turn(cx.tx, &capture.role)? {
        return Err(Error::rejected("turn_unfinished", format!("{} has unfinished turn {}", capture.role, turn.id)));
    }
    let detail = json!({ "capture_id": capture.id, "options": options });
    let kind = if keep { "approve_new_files" } else { "discard_uncaptured" };
    record_decision(cx, kind, capture.task_id.as_deref(), caller, detail)?;
    // A stopped done kept its mark on the attempt, so the new result is the candidate if pinned.
    cx.tx.execute(
        "UPDATE capture SET state = 'intent', options = ?2, detail = NULL, finished_at = NULL WHERE id = ?1",
        params![capture.id, serde_json::to_string(&options)?],
    )?;
    cx.emit("capture.retry", &capture.id, json!({ "options": options }))?;
    Ok(())
}

/// "放行新增文件": the new files over the guardrail belong in the result.
#[derive(Debug, Serialize, Deserialize)]
pub struct ApproveNewFiles {
    pub request_id: String,
    pub capture_id: String,
}

impl Command for ApproveNewFiles {
    const NAME: &'static str = "approve_new_files";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        retry(cx, caller, &self.capture_id, true)
    }
}

/// "丢弃未能采集的内容": what stopped the capture is left out, the rest is captured. For the
/// guardrail that is the new files; otherwise the content a capture cannot represent. Left-out
/// files stay in the slot until it is materialized again.
#[derive(Debug, Serialize, Deserialize)]
pub struct DiscardUncaptured {
    pub request_id: String,
    pub capture_id: String,
}

impl Command for DiscardUncaptured {
    const NAME: &'static str = "discard_uncaptured";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        retry(cx, caller, &self.capture_id, false)
    }
}
