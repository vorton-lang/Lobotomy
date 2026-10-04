//! Slots: the directories roles work in, materialized from the private store
//! (harness-adapter.md §3; data-model.md §5, §6).
//!
//! The record is the intent: the runtime writes the target in the same transaction as the
//! business change that needs it, then writes the directory, then marks it ready. Turns only run
//! in a ready slot.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::id::new_id;
use crate::project::require_project;
use crate::sql::sql_enum;

sql_enum! {
    pub enum WorkspaceState {
        Materializing = "materializing",
        Ready = "ready",
        Retired = "retired",
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub generation: i64,
    /// The commit written into the directory.
    pub target: String,
    /// git's HEAD in the directory: the baseline.
    pub head: String,
    /// The task the slot was written for; `None` for a role without a task.
    pub task_id: Option<String>,
    pub state: WorkspaceState,
}

const SELECT: &str = "SELECT id, name, generation, target, head, task_id, state FROM workspace";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: r.get(0)?,
        name: r.get(1)?,
        generation: r.get(2)?,
        target: r.get(3)?,
        head: r.get(4)?,
        task_id: r.get(5)?,
        state: r.get(6)?,
    })
}

fn query(conn: &Connection, filter: &str, args: impl rusqlite::Params) -> Result<Vec<Workspace>> {
    let mut stmt = conn.prepare(&format!("{SELECT} {filter}"))?;
    let rows = stmt.query_map(args, from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn load_workspace(conn: &Connection, id: &str) -> Result<Workspace> {
    query(conn, "WHERE id = ?1", [id])?.pop().ok_or_else(|| Error::rejected("not_found", format!("no workspace {id}")))
}

/// The slot's current generation, if it was ever materialized.
pub fn current_workspace(conn: &Connection, name: &str) -> Result<Option<Workspace>> {
    Ok(query(conn, "WHERE name = ?1 AND state <> 'retired'", [name])?.pop())
}

/// Slots waiting to be written.
pub fn materializing(conn: &Connection) -> Result<Vec<Workspace>> {
    query(conn, "WHERE state = 'materializing' ORDER BY created_at, id", [])
}

pub fn role_slot(conn: &Connection, role: &str) -> Result<Option<String>> {
    conn.query_row("SELECT slot FROM role WHERE name = ?1", [role], |r| r.get(0))
        .optional()?
        .ok_or_else(|| Error::rejected("unknown_role", format!("no role {role}")))
}

/// Records that the slot must hold `target` with git's HEAD at `head`, for the work of `task_id`
/// (`None`: a role without a task). The caller has made sure the slot's current content was
/// captured (harness-adapter.md §3).
pub(crate) fn plan(cx: &mut Cx<'_>, name: &str, target: &str, head: &str, task_id: Option<&str>) -> Result<String> {
    let id = match current_workspace(cx.tx, name)? {
        Some(ws) => {
            cx.tx.execute(
                "UPDATE workspace SET target = ?2, head = ?3, task_id = ?4, state = 'materializing', ready_at = NULL
                 WHERE id = ?1",
                params![ws.id, target, head, task_id],
            )?;
            ws.id
        }
        None => {
            let id = new_id("ws");
            cx.tx.execute(
                "INSERT INTO workspace (id, name, generation, target, head, task_id, state, created_at)
                 VALUES (?1, ?2, 1, ?3, ?4, ?5, 'materializing', ?6)",
                params![id, name, target, head, task_id, cx.now],
            )?;
            id
        }
    };
    cx.emit(
        "workspace.materializing",
        &id,
        json!({ "name": name, "target": target, "head": head, "task_id": task_id }),
    )?;
    Ok(id)
}

/// Whether the role's slot may be written with other content (harness-adapter.md §3): no turn runs
/// in it, its content is captured, and no changes made outside any task wait for the user (#14).
/// The user's own decision about such changes passes their capture as `deciding`. Every way of
/// planning another target over a slot in use checks this here, so the rules cannot drift apart
/// (#16).
pub fn require_slot_free(conn: &Connection, role: &str, deciding: Option<&str>) -> Result<()> {
    if let Some(turn) = crate::turn::unfinished_turn(conn, role)? {
        return Err(Error::rejected("turn_unfinished", format!("{role} has unfinished turn {}", turn.id)));
    }
    crate::capture::require_captured(conn, role)?;
    if let Some(capture) = crate::capture::outside_changes(conn, role)?
        && deciding != Some(capture.id.as_str())
    {
        return Err(Error::rejected(
            "outside_changes",
            format!("the slot of {role} holds undecided changes of capture {}", capture.id),
        ));
    }
    Ok(())
}

/// The slot follows the role's work: with a task it holds the attempt's code start, without one
/// the integration version. When the role has no task and its slot holds something else (it was
/// never written, or the last task was accepted or abandoned), the slot is planned at the
/// integration version, once it is free (harness-adapter.md §3).
///
/// Changes made in turns without a task stay in the slot until the user makes a task of them or
/// discards them (#14).
#[derive(Debug, Serialize, Deserialize)]
pub struct AlignIdleSlot {
    pub role: String,
}

impl Command for AlignIdleSlot {
    const NAME: &'static str = "align_idle_slot";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let slot = role_slot(cx.tx, &self.role)?
            .ok_or_else(|| Error::rejected("no_slot", format!("{} has no slot", self.role)))?;
        if let Some(task) = crate::task::occupant(cx.tx, &self.role)? {
            return Err(Error::rejected("role_busy", format!("{} works on {task}", self.role)));
        }
        let project = require_project(cx.tx)?;
        if let Some(ws) = current_workspace(cx.tx, &slot)?
            && ws.task_id.is_none()
            && ws.target == project.integration
            && ws.head == project.integration
        {
            return Err(Error::rejected("slot_aligned", format!("slot {slot} is at the integration version")));
        }
        require_slot_free(cx.tx, &self.role, None)?;
        plan(cx, &slot, &project.integration, &project.integration, None)?;
        Ok(())
    }
}

/// The directory now holds the target.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspaceReady {
    pub workspace_id: String,
    pub target: String,
    pub head: String,
}

impl Command for WorkspaceReady {
    const NAME: &'static str = "workspace_ready";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let ws = load_workspace(cx.tx, &self.workspace_id)?;
        // The target may have changed while the directory was written; then it is written again.
        if ws.state != WorkspaceState::Materializing || ws.target != self.target || ws.head != self.head {
            return Err(Error::rejected("stale_materialization", format!("workspace {} wants {}", ws.id, ws.target)));
        }
        cx.tx.execute("UPDATE workspace SET state = 'ready', ready_at = ?2 WHERE id = ?1", params![ws.id, cx.now])?;
        cx.emit("workspace.ready", &ws.id, json!({ "name": ws.name, "generation": ws.generation }))?;
        Ok(())
    }
}

/// The directory could not be brought to its target. The runtime has moved it aside; the next
/// generation starts from an empty directory (harness-adapter.md §3 case 2).
#[derive(Debug, Serialize, Deserialize)]
pub struct ReplaceWorkspace {
    pub workspace_id: String,
    pub reason: String,
}

impl Command for ReplaceWorkspace {
    const NAME: &'static str = "replace_workspace";
    type Output = Workspace;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<Workspace> {
        caller.require_runtime()?;
        let old = load_workspace(cx.tx, &self.workspace_id)?;
        if old.state != WorkspaceState::Materializing {
            return Err(Error::rejected("not_materializing", format!("workspace {} is not being written", old.id)));
        }
        cx.tx.execute("UPDATE workspace SET state = 'retired' WHERE id = ?1", [&old.id])?;
        let new =
            Workspace { id: new_id("ws"), generation: old.generation + 1, state: WorkspaceState::Materializing, ..old };
        cx.tx.execute(
            "INSERT INTO workspace (id, name, generation, target, head, task_id, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'materializing', ?7)",
            params![new.id, new.name, new.generation, new.target, new.head, new.task_id, cx.now],
        )?;
        cx.emit(
            "workspace.replaced",
            &new.id,
            json!({ "name": new.name, "generation": new.generation, "reason": self.reason }),
        )?;
        Ok(new)
    }
}
