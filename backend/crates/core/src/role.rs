//! Named roles (roles-and-tasks.md §1). v1 configures them statically.

use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Role {
    pub name: String,
    pub kind: String,
    pub harness: String,
    pub model: Option<String>,
    pub slot: Option<String>,
}

fn role_from_row(r: &Row<'_>) -> rusqlite::Result<Role> {
    Ok(Role { name: r.get(0)?, kind: r.get(1)?, harness: r.get(2)?, model: r.get(3)?, slot: r.get(4)? })
}

pub fn load_role(conn: &Connection, name: &str) -> Result<Role> {
    conn.query_row("SELECT name, kind, harness, model, slot FROM role WHERE name = ?1", [name], role_from_row)
        .optional()?
        .ok_or_else(|| Error::rejected("unknown_role", format!("no role {name}")))
}

pub fn list_roles(conn: &Connection) -> Result<Vec<Role>> {
    let mut stmt = conn.prepare("SELECT name, kind, harness, model, slot FROM role ORDER BY name")?;
    let rows = stmt.query_map([], role_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
