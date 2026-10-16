//! The host database: state shared by all projects of this user, outside any project
//! (data-model.md §10). It holds the project registry (§10.3), the quota domains (§8) and each
//! harness's permission mode (harness-adapter.md §1.9).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::error::{Error, Result};

/// Applied like a project's migrations (`db.rs`): numbered, each once, in its own transaction.
const MIGRATIONS: &[&str] =
    &[include_str!("../host_migrations/0001_init.sql"), include_str!("../host_migrations/0002_projects.sql")];

pub struct HostDb {
    conn: Mutex<Connection>,
}

/// A registered project (data-model.md §10.3).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct ProjectEntry {
    /// The same as the project row's id in the project's database.
    pub id: String,
    pub name: String,
    pub data_dir: String,
    /// Copied at registration and never changed; the project's database is authoritative.
    pub repo_path: String,
    pub state: ProjectState,
    pub registered_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum ProjectState {
    /// Registered, its repository not connected yet. Startup removes what an interrupted
    /// onboarding left (data-model.md §10.4).
    Onboarding,
    Running,
    Archived,
}

impl ProjectState {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectState::Onboarding => "onboarding",
            ProjectState::Running => "running",
            ProjectState::Archived => "archived",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "onboarding" => ProjectState::Onboarding,
            "running" => ProjectState::Running,
            "archived" => ProjectState::Archived,
            other => return Err(Error::invariant(format!("unknown project state {other:?} in the host database"))),
        })
    }
}

impl HostDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        crate::db::migrate(&mut conn, MIGRATIONS)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The harness's permission mode, if the user set one.
    pub fn permission(&self, harness: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT permission FROM harness_setting WHERE harness = ?1", [harness], |r| r.get(0))
            .optional()?)
    }

    pub fn set_permission(&self, harness: &str, permission: &str, now: i64) -> Result<()> {
        self.conn().execute(
            "INSERT INTO harness_setting (harness, permission, changed_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (harness) DO UPDATE SET permission = excluded.permission, changed_at = excluded.changed_at",
            params![harness, permission, now],
        )?;
        Ok(())
    }

    /// The registered projects, oldest first.
    pub fn projects(&self) -> Result<Vec<ProjectEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, data_dir, repo_path, state, registered_at FROM project ORDER BY registered_at, id",
        )?;
        let rows = stmt.query_map([], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(entry, state)| Ok(ProjectEntry { state: ProjectState::parse(&state)?, ..entry }))
            .collect()
    }

    pub fn project(&self, id: &str) -> Result<Option<ProjectEntry>> {
        let found = self
            .conn()
            .query_row(
                "SELECT id, name, data_dir, repo_path, state, registered_at FROM project WHERE id = ?1",
                [id],
                row,
            )
            .optional()?;
        found.map(|(entry, state)| Ok(ProjectEntry { state: ProjectState::parse(&state)?, ..entry })).transpose()
    }

    /// Registers a project. Refused while its repository has an unarchived project (§10.3).
    pub fn register(&self, entry: &ProjectEntry) -> Result<()> {
        let conn = self.conn();
        if entry.state != ProjectState::Archived {
            unarchived_elsewhere(&conn, &entry.repo_path, &entry.id)?;
        }
        conn.execute(
            "INSERT INTO project (id, name, data_dir, repo_path, state, registered_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![entry.id, entry.name, entry.data_dir, entry.repo_path, entry.state.as_str(), entry.registered_at],
        )?;
        Ok(())
    }

    /// Moves a project to another state. Leaving the archive is refused while its repository has
    /// another unarchived project (§10.4).
    pub fn set_project_state(&self, id: &str, state: ProjectState) -> Result<()> {
        let conn = self.conn();
        let repo: Option<String> =
            conn.query_row("SELECT repo_path FROM project WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        let Some(repo) = repo else {
            return Err(Error::rejected("unknown_project", format!("no project {id}")));
        };
        if state != ProjectState::Archived {
            unarchived_elsewhere(&conn, &repo, id)?;
        }
        conn.execute("UPDATE project SET state = ?2 WHERE id = ?1", params![id, state.as_str()])?;
        Ok(())
    }

    /// Removes a registration. Only an onboarding that did not finish is removed (§10.4); the GUI
    /// offers no way to remove a project.
    pub fn unregister_onboarding(&self, id: &str) -> Result<()> {
        self.conn().execute("DELETE FROM project WHERE id = ?1 AND state = 'onboarding'", [id])?;
        Ok(())
    }
}

type Row = (ProjectEntry, String);

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok((
        ProjectEntry {
            id: r.get(0)?,
            name: r.get(1)?,
            data_dir: r.get(2)?,
            repo_path: r.get(3)?,
            state: ProjectState::Archived,
            registered_at: r.get(5)?,
        },
        r.get(4)?,
    ))
}

fn unarchived_elsewhere(conn: &Connection, repo_path: &str, id: &str) -> Result<()> {
    let other: Option<String> = conn
        .query_row(
            "SELECT name FROM project WHERE repo_path = ?1 AND state <> 'archived' AND id <> ?2",
            params![repo_path, id],
            |r| r.get(0),
        )
        .optional()?;
    match other {
        Some(name) => Err(Error::rejected("repo_in_use", format!("仓库 {repo_path} 已经接入了项目「{name}」"))),
        None => Ok(()),
    }
}
