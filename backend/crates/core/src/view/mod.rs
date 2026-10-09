//! What the GUI reads (frontend.md §7): typed read models, serialized as they are. Each is read in
//! one call of `Db::read`, so it sees one committed state (#16).
//!
//! The GUI's connection in `lobotomyd` adds what only the running backend knows: the live items of
//! running turns, the host's quota domains and settings, failed store jobs, and whether a process
//! still runs.

mod task;
mod thread;

use std::collections::HashMap;

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use crate::capture::{Capture, capture_in_progress, outside_changes};
use crate::error::Result;
use crate::project::{Project, ProjectConfig, current_config, load_project};
use crate::quota::Domain;
use crate::role::{Role, list_roles};
use crate::task::{
    Phase, Task, latest_attempt, list_tasks, load_task, occupant, open_attempt, queued_messages,
    undelivered_user_messages,
};
use crate::turn::{Hold, Outcome, Turn, TurnState, hold, last_turn, unfinished_turn};
use crate::verify::{Verification, latest_verification, preview_stopped};
use crate::workspace::{Workspace, current_workspace};

pub use task::{CheckRun, Criteria, Decision, Publication, TaskDetail, task_detail};
pub use thread::{
    CommandRecord, Found, Item, Match, MatchKind, SearchQuery, ThreadPage, ThreadQuery, search, thread_page,
};

/// What the GUI's snapshot reads from the database.
#[derive(Serialize)]
pub struct Overview {
    /// The last event it includes; pushed events after it apply on top.
    pub seq: i64,
    pub project: Option<Project>,
    pub config: Option<ConfigView>,
    pub roles: Vec<RoleView>,
    pub tasks: Vec<TaskView>,
    /// Why the preview stopped, if it did. The GUI sees it as an attention entry.
    #[serde(skip)]
    pub preview_stopped: Option<String>,
}

#[derive(Serialize)]
pub struct ConfigView {
    pub version: i64,
    pub config: ProjectConfig,
}

#[derive(Serialize)]
pub struct RoleView {
    #[serde(flatten)]
    pub role: Role,
    pub task_id: Option<String>,
    pub unfinished: Option<Turn>,
    pub last_turn: Option<Turn>,
    pub hold: Option<Hold>,
    pub stalled: Option<Stalled>,
    /// Changes a turn outside any task left in the slot, waiting for the user (#14).
    pub outside: Option<Capture>,
    pub workspace: Option<Workspace>,
    pub queued_messages: usize,
}

#[derive(Clone, Serialize)]
pub struct Stalled {
    pub task_id: String,
    pub turn_id: String,
    /// Why a call to a Lobotomy tool in that turn recorded nothing, when one did not.
    pub report_error: Option<String>,
}

#[derive(Serialize)]
pub struct TaskView {
    #[serde(flatten)]
    pub task: Task,
    pub attempt_seq: Option<i64>,
    pub attempt_open: bool,
    pub verification: Option<Verification>,
    /// The user's messages its executor never got; accepting needs the user to let them go (#14).
    pub undelivered_messages: usize,
}

/// `mcp_server` is the name Lobotomy's tools have in the harness's configuration: its calls that
/// recorded nothing tell why a role stalled.
pub fn overview(conn: &Connection, mcp_server: &str) -> Result<Overview> {
    let seq = last_event(conn)?;
    let project = load_project(conn)?;
    let config = match project {
        Some(_) => Some(current_config(conn).map(|(version, config)| ConfigView { version, config })?),
        None => None,
    };
    let mut roles = Vec::new();
    for role in list_roles(conn)? {
        let workspace = match &role.slot {
            Some(slot) => current_workspace(conn, slot)?,
            None => None,
        };
        let mut view = RoleView {
            task_id: occupant(conn, &role.name)?,
            unfinished: unfinished_turn(conn, &role.name)?,
            last_turn: last_turn(conn, &role.name)?,
            hold: hold(conn, &role.name)?,
            stalled: None,
            outside: outside_changes(conn, &role.name)?,
            workspace,
            queued_messages: queued_messages(conn, &role.name)?.len(),
            role,
        };
        view.stalled = stalled(conn, &view, mcp_server)?;
        roles.push(view);
    }
    let mut tasks = Vec::new();
    for task in list_tasks(conn)? {
        let attempt = latest_attempt(conn, &task.id)?;
        tasks.push(TaskView {
            attempt_seq: attempt.as_ref().map(|a| a.seq),
            attempt_open: attempt.is_some_and(|a| a.ended_at.is_none()),
            verification: latest_verification(conn, &task.id)?,
            undelivered_messages: undelivered_user_messages(conn, &task.id)?.len(),
            task,
        });
    }
    let preview_stopped = if project.is_some() { preview_stopped(conn)? } else { None };
    Ok(Overview { seq, project, config, roles, tasks, preview_stopped })
}

/// The role's task is executing, yet nothing will move it on without the user: the last turn of
/// the attempt completed without done or a question, and nothing is queued, running, held or
/// being captured (#13).
fn stalled(conn: &Connection, role: &RoleView, mcp_server: &str) -> Result<Option<Stalled>> {
    let (Some(task_id), Some(last)) = (&role.task_id, &role.last_turn) else { return Ok(None) };
    if role.unfinished.is_some()
        || role.hold.is_some()
        || role.queued_messages > 0
        || capture_in_progress(conn, &role.role.name)?.is_some()
    {
        return Ok(None);
    }
    let task = load_task(conn, task_id)?;
    let attempt = open_attempt(conn, task_id)?;
    if task.phase != Phase::Executing
        || task.paused
        || task.blocked_reason.is_some()
        || last.outcome != Some(Outcome::Completed)
        || last.attempt_id != attempt.map(|a| a.id)
    {
        return Ok(None);
    }
    // A Lobotomy tool call that went through has a command record; one without failed before
    // reaching the runtime, or the runtime refused it.
    let mut stmt = conn.prepare(
        "SELECT content FROM item WHERE turn_id = ?1 AND kind = 'mcp_call' AND command_id IS NULL ORDER BY seq",
    )?;
    let calls = stmt.query_map([&last.id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let report_error = calls
        .iter()
        .filter_map(|c| serde_json::from_str::<Value>(c).ok())
        .filter(|c| c["server"] == mcp_server)
        // The adapter says why in harness-neutral fields (harness/src/event.rs).
        .map(|c| match (c["error_text"].as_str(), c["result_text"].as_str()) {
            (Some(error), _) => error.to_owned(),
            (None, Some(reply)) => reply.to_owned(),
            (None, None) => "调用没有返回内容".to_owned(),
        })
        .next_back();
    Ok(Some(Stalled { task_id: task_id.clone(), turn_id: last.id.clone(), report_error }))
}

/// Something that waits for the user ("等你决定", frontend.md §2).
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attention {
    /// The role waits: continue, start a new session, or decide on a stopped capture.
    Hold {
        role: String,
        hold: Hold,
    },
    /// A turn outside any task changed files; no task starts in the slot until the user makes a
    /// task of them or discards them (#14).
    OutsideChanges {
        role: String,
        capture: Capture,
    },
    /// The backend restarted while this turn's CLI ran, and the CLI still runs.
    UnknownTurn {
        role: String,
        turn_id: String,
    },
    /// The executor asked the user a question, and no reply from the user waits in the queue.
    TaskBlocked {
        task_id: String,
        title: String,
        reason: String,
    },
    /// The executor's turn ended normally but it reported neither done nor a question, and
    /// nothing is queued for it. In M1 only the user can move it on (#13).
    Stalled {
        role: String,
        task_id: String,
        title: String,
        turn_id: String,
        report_error: Option<String>,
    },
    Accept {
        task_id: String,
        title: String,
        verification_id: String,
    },
    Quota {
        domain: Domain,
    },
    JobFailed {
        key: String,
        reason: String,
    },
    PreviewStopped {
        reason: String,
    },
}

/// What waits for the user, in the order the GUI lists it: each role's, each task's, blocked
/// quota domains, failed store jobs (`failed`, by key with the reason) and a stopped preview.
/// `still_runs` tells whether an unknown turn's CLI is still alive.
pub fn attention(
    overview: &Overview,
    quota: &[Domain],
    failed: &[(String, String)],
    still_runs: impl Fn(&Turn) -> bool,
) -> Vec<Attention> {
    let mut out = Vec::new();
    for role in &overview.roles {
        let name = &role.role.name;
        if let Some(turn) = &role.unfinished
            && turn.state == TurnState::Unknown
            && still_runs(turn)
        {
            out.push(Attention::UnknownTurn { role: name.clone(), turn_id: turn.id.clone() });
        }
        if let Some(hold) = &role.hold {
            out.push(Attention::Hold { role: name.clone(), hold: hold.clone() });
        }
        if let Some(capture) = &role.outside {
            out.push(Attention::OutsideChanges { role: name.clone(), capture: capture.clone() });
        }
        if let Some(stalled) = &role.stalled {
            let task = overview.tasks.iter().find(|t| t.task.id == stalled.task_id).map(|t| &t.task);
            out.push(Attention::Stalled {
                role: name.clone(),
                task_id: stalled.task_id.clone(),
                title: task.map(|t| t.title.clone()).unwrap_or_default(),
                turn_id: stalled.turn_id.clone(),
                report_error: stalled.report_error.clone(),
            });
        }
    }
    for view in &overview.tasks {
        let t = &view.task;
        // After the user replies, the reply waits in the queue for the executor's next turn. The
        // user has nothing more to do; registering the turn clears the question (#17).
        if let (Phase::Executing, Some(reason)) = (t.phase, &t.blocked_reason)
            && view.undelivered_messages == 0
        {
            out.push(Attention::TaskBlocked { task_id: t.id.clone(), title: t.title.clone(), reason: reason.clone() });
        }
        if t.phase == Phase::Accepting
            && let Some(v) = &view.verification
        {
            out.push(Attention::Accept {
                task_id: t.id.clone(),
                title: t.title.clone(),
                verification_id: v.id.clone(),
            });
        }
    }
    out.extend(quota.iter().filter(|d| d.is_blocked()).map(|d| Attention::Quota { domain: d.clone() }));
    out.extend(failed.iter().map(|(key, reason)| Attention::JobFailed { key: key.clone(), reason: reason.clone() }));
    if let Some(reason) = &overview.preview_stopped {
        out.push(Attention::PreviewStopped { reason: reason.clone() });
    }
    out
}

// ---- what is pushed ----

/// An entry of the global event log.
#[derive(Serialize)]
pub struct Event {
    pub seq: i64,
    pub kind: String,
    pub entity: String,
    pub payload: Value,
}

/// Events one push carries at most; the rest follow in the next.
const EVENTS_PER_PUSH: i64 = 500;

pub fn last_event(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM event", [], |r| r.get(0))?)
}

pub fn events_after(conn: &Connection, seq: i64) -> Result<Vec<Event>> {
    let mut stmt = conn.prepare("SELECT seq, kind, entity, payload FROM event WHERE seq > ?1 ORDER BY seq LIMIT ?2")?;
    let rows =
        stmt.query_map([seq, EVENTS_PER_PUSH], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, String>(3)?)))?;
    rows.map(|row| {
        let (seq, kind, entity, payload) = row?;
        Ok(Event { seq, kind, entity, payload: serde_json::from_str(&payload)? })
    })
    .collect()
}

/// The newest item of each role's thread.
pub fn thread_heads(conn: &Connection) -> Result<HashMap<String, i64>> {
    let mut stmt =
        conn.prepare("SELECT t.role, MAX(i.seq) FROM item i JOIN thread t ON t.id = i.thread_id GROUP BY t.role")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
