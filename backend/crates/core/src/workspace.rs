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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceState {
    Materializing,
    Ready,
    Retired,
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
    pub state: WorkspaceState,
}

const SELECT: &str = "SELECT id, name, generation, target, head, state FROM workspace";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Workspace, String)> {
    Ok((
        Workspace {
            id: r.get(0)?,
            name: r.get(1)?,
            generation: r.get(2)?,
            target: r.get(3)?,
            head: r.get(4)?,
            state: WorkspaceState::Ready,
        },
        r.get(5)?,
    ))
}

fn parse((mut ws, state): (Workspace, String)) -> Result<Workspace> {
    ws.state = match state.as_str() {
        "materializing" => WorkspaceState::Materializing,
        "ready" => WorkspaceState::Ready,
        "retired" => WorkspaceState::Retired,
        other => return Err(Error::rejected("bad_state", format!("unknown workspace state {other}"))),
    };
    Ok(ws)
}

fn query(conn: &Connection, filter: &str, args: impl rusqlite::Params) -> Result<Vec<Workspace>> {
    let mut stmt = conn.prepare(&format!("{SELECT} {filter}"))?;
    let rows = stmt.query_map(args, from_row)?;
    rows.map(|r| parse(r?)).collect()
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

/// Records that the slot must hold `target` with git's HEAD at `head`. The caller has made sure
/// the slot's current content was captured (harness-adapter.md §3).
pub(crate) fn plan(cx: &mut Cx<'_>, name: &str, target: &str, head: &str) -> Result<String> {
    let id = match current_workspace(cx.tx, name)? {
        Some(ws) => {
            cx.tx.execute(
                "UPDATE workspace SET target = ?2, head = ?3, state = 'materializing', ready_at = NULL WHERE id = ?1",
                params![ws.id, target, head],
            )?;
            ws.id
        }
        None => {
            let id = new_id("ws");
            cx.tx.execute(
                "INSERT INTO workspace (id, name, generation, target, head, state, created_at)
                 VALUES (?1, ?2, 1, ?3, ?4, 'materializing', ?5)",
                params![id, name, target, head, cx.now],
            )?;
            id
        }
    };
    cx.emit("workspace.materializing", &id, json!({ "name": name, "target": target, "head": head }))?;
    Ok(id)
}

/// Materializes a role's slot at the integration version when it has none, so that a role
/// without a task can still take a turn.
#[derive(Debug, Serialize, Deserialize)]
pub struct PrepareSlot {
    pub role: String,
}

impl Command for PrepareSlot {
    const NAME: &'static str = "prepare_slot";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let slot = role_slot(cx.tx, &self.role)?
            .ok_or_else(|| Error::rejected("no_slot", format!("{} has no slot", self.role)))?;
        if current_workspace(cx.tx, &slot)?.is_some() {
            return Err(Error::rejected("slot_exists", format!("slot {slot} exists")));
        }
        let project = require_project(cx.tx)?;
        plan(cx, &slot, &project.integration, &project.integration)?;
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
        let new = Workspace { id: new_id("ws"), generation: old.generation + 1, state: WorkspaceState::Materializing, ..old };
        cx.tx.execute(
            "INSERT INTO workspace (id, name, generation, target, head, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'materializing', ?6)",
            params![new.id, new.name, new.generation, new.target, new.head, cx.now],
        )?;
        cx.emit(
            "workspace.replaced",
            &new.id,
            json!({ "name": new.name, "generation": new.generation, "reason": self.reason }),
        )?;
        Ok(new)
    }
}
