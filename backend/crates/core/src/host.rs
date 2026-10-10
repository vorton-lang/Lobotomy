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

#[cfg(test)]
mod tests {
    use super::*;

    /// Host databases made before migrations existed have the tables and no version (#16).
    #[test]
    fn a_host_database_from_before_migrations_keeps_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.execute("INSERT INTO harness_setting VALUES ('codex', 'auto_review', 1)", []).unwrap();
        }
        let host = HostDb::open(&path).unwrap();
        assert_eq!(host.permission("codex").unwrap().as_deref(), Some("auto_review"));
        let version: i64 = host.conn().pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    #[test]
    fn each_harness_keeps_its_own_permission() {
        let host = HostDb::open_in_memory().unwrap();
        assert_eq!(host.permission("codex").unwrap(), None);
        host.set_permission("codex", "auto_review", 1).unwrap();
        assert_eq!(host.permission("codex").unwrap().as_deref(), Some("auto_review"));
        assert_eq!(host.permission("claude").unwrap(), None, "the other harness keeps its default");
        host.set_permission("codex", "full", 2).unwrap();
        assert_eq!(host.permission("codex").unwrap().as_deref(), Some("full"));
    }

    fn entry(id: &str, repo: &str, state: ProjectState) -> ProjectEntry {
        ProjectEntry {
            id: id.into(),
            name: id.into(),
            data_dir: format!("/data/{id}"),
            repo_path: repo.into(),
            state,
            registered_at: 1,
        }
    }

    /// A repository has at most one unarchived project. An archived one does not hold it, so a
    /// project whose data directory is lost can be archived and the repository connected again
    /// (data-model.md §10.3).
    #[test]
    fn a_repository_has_one_unarchived_project() {
        let host = HostDb::open_in_memory().unwrap();
        host.register(&entry("p1", "/repo", ProjectState::Onboarding)).unwrap();
        let second = host.register(&entry("p2", "/repo", ProjectState::Onboarding));
        assert_eq!(second.unwrap_err().code(), Some("repo_in_use"));
        host.register(&entry("p3", "/other", ProjectState::Onboarding)).unwrap();

        host.set_project_state("p1", ProjectState::Archived).unwrap();
        host.register(&entry("p2", "/repo", ProjectState::Onboarding)).unwrap();
        host.set_project_state("p2", ProjectState::Running).unwrap();
        // The archived project cannot come back while the new one holds the repository.
        let back = host.set_project_state("p1", ProjectState::Running);
        assert_eq!(back.unwrap_err().code(), Some("repo_in_use"));
        assert_eq!(host.project("p1").unwrap().unwrap().state, ProjectState::Archived);

        let states: Vec<_> = host.projects().unwrap().into_iter().map(|p| (p.id, p.state)).collect();
        assert_eq!(
            states,
            [
                ("p1".into(), ProjectState::Archived),
                ("p2".into(), ProjectState::Running),
                ("p3".into(), ProjectState::Onboarding)
            ]
        );
    }

    #[test]
    fn only_an_unfinished_onboarding_is_unregistered() {
        let host = HostDb::open_in_memory().unwrap();
        host.register(&entry("p1", "/repo", ProjectState::Onboarding)).unwrap();
        host.register(&entry("p2", "/other", ProjectState::Running)).unwrap();
        host.unregister_onboarding("p1").unwrap();
        host.unregister_onboarding("p2").unwrap();
        let ids: Vec<_> = host.projects().unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["p2"]);
    }
}
