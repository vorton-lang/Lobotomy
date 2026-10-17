//! Named roles (roles-and-tasks.md §1). v1 configures them statically.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};

/// The harnesses a role can be on, as the role table allows them.
pub const HARNESSES: [&str; 2] = ["claude", "codex"];

#[derive(Clone, Debug, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct Role {
    pub name: String,
    /// As the role table's check allows.
    #[cfg_attr(feature = "ts-rs", ts(type = "\"manager\" | \"tech_lead\" | \"worker\" | \"reviewer\""))]
    pub kind: String,
    pub harness: String,
    /// The model chosen for the role on its harness; `None` leaves it to the CLI's default.
    pub model: Option<String>,
    /// The reasoning effort chosen likewise.
    pub effort: Option<String>,
    /// A worker plays it, outside the backend (tech-plan.md §7 step 1): its tasks go to the worker.
    pub remote: bool,
}

const ROLE_SELECT: &str = "SELECT r.name, r.kind, r.harness, m.model, m.effort, r.remote FROM role r
                           LEFT JOIN role_model m ON m.role = r.name AND m.harness = r.harness";

fn role_from_row(r: &Row<'_>) -> rusqlite::Result<Role> {
    Ok(Role {
        name: r.get(0)?,
        kind: r.get(1)?,
        harness: r.get(2)?,
        model: r.get(3)?,
        effort: r.get(4)?,
        remote: r.get::<_, i64>(5)? != 0,
    })
}

pub fn load_role(conn: &Connection, name: &str) -> Result<Role> {
    conn.query_row(&format!("{ROLE_SELECT} WHERE r.name = ?1"), [name], role_from_row)
        .optional()?
        .ok_or_else(|| Error::rejected("unknown_role", format!("no role {name}")))
}

pub fn list_roles(conn: &Connection) -> Result<Vec<Role>> {
    let mut stmt = conn.prepare(&format!("{ROLE_SELECT} ORDER BY r.name"))?;
    let rows = stmt.query_map([], role_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Chooses the model and reasoning effort a role uses on a harness (harness-adapter.md §1.2);
/// `None` leaves either to the CLI's default. Model names belong to a harness, so each harness
/// keeps its own choice. Every turn reads the choice when it starts: a running turn keeps what it
/// started with, and the next turn resumes the same session with the new choice.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct SetRoleModel {
    pub request_id: String,
    pub role: String,
    pub harness: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl Command for SetRoleModel {
    const NAME: &'static str = "set_role_model";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user()?;
        load_role(cx.tx, &self.role)?;
        if !HARNESSES.contains(&self.harness.as_str()) {
            return Err(Error::rejected("unknown_harness", format!("unknown harness {}", self.harness)));
        }
        if let Some(value) = [&self.model, &self.effort].into_iter().flatten().find(|v| !is_cli_value(v)) {
            return Err(Error::rejected("invalid_model", format!("{value:?} is not a model or effort name")));
        }
        cx.tx.execute(
            "INSERT INTO role_model (role, harness, model, effort) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (role, harness) DO UPDATE SET model = excluded.model, effort = excluded.effort",
            params![self.role, self.harness, self.model, self.effort],
        )?;
        let payload = json!({ "harness": self.harness, "model": self.model, "effort": self.effort });
        cx.emit("role.model_changed", &self.role, payload)?;
        Ok(())
    }
}

/// A model or an effort goes on the CLI's command line as an option's value, so it must not read
/// as an option itself. Names such as `claude-opus-5-5[1m]` and `gpt-6.1-sol` pass.
fn is_cli_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && !value.starts_with('-')
        && value.chars().all(|c| c.is_ascii_alphanumeric() || "._-:[]/".contains(c))
}

/// The project's workers, as the host's configuration lists them (tech-plan.md §7 step 1): each
/// gets a remote role of its name; a remote role no longer listed stops taking tasks. Roles the
/// backend used to run itself, such as the seeded executor, go while nothing refers to them.
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncWorkerRoles {
    /// Each worker's name and harness.
    pub workers: Vec<(String, String)>,
}

impl Command for SyncWorkerRoles {
    const NAME: &'static str = "sync_worker_roles";
    type Output = ();

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let listed: Vec<&str> = self.workers.iter().map(|(name, _)| name.as_str()).collect();
        for role in list_roles(cx.tx)?.into_iter().filter(|r| !r.remote && !listed.contains(&r.name.as_str())) {
            let referred: bool = cx.tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM native_session WHERE role = ?1) OR EXISTS (SELECT 1 FROM message WHERE role = ?1)
                     OR EXISTS (SELECT 1 FROM task WHERE executor = ?1) OR EXISTS (SELECT 1 FROM thread WHERE role = ?1)",
                [&role.name],
                |r| r.get(0),
            )?;
            if !referred {
                cx.tx.execute("DELETE FROM role_model WHERE role = ?1", [&role.name])?;
                cx.tx.execute("DELETE FROM role WHERE name = ?1", [&role.name])?;
                cx.emit("role.removed", &role.name, json!({}))?;
            }
        }
        for role in list_roles(cx.tx)?.into_iter().filter(|r| r.remote && !listed.contains(&r.name.as_str())) {
            cx.tx.execute("UPDATE role SET remote = 0 WHERE name = ?1", [&role.name])?;
            cx.emit("role.worker_removed", &role.name, json!({}))?;
        }
        for (name, harness) in &self.workers {
            if !HARNESSES.contains(&harness.as_str()) {
                return Err(Error::rejected("unknown_harness", format!("unknown harness {harness}")));
            }
            let current = cx
                .tx
                .query_row("SELECT harness, remote FROM role WHERE name = ?1", [name], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))
                })
                .optional()?;
            if current.as_ref().is_some_and(|(h, remote)| h == harness && *remote) {
                continue;
            }
            cx.tx.execute(
                "INSERT INTO role (name, kind, harness, remote) VALUES (?1, 'worker', ?2, 1)
                 ON CONFLICT (name) DO UPDATE SET harness = excluded.harness, remote = 1",
                params![name, harness],
            )?;
            cx.emit("role.worker_added", name, json!({ "harness": harness }))?;
        }
        Ok(())
    }
}
