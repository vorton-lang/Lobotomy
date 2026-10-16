//! The project and the repository it is for (data-model.md §10).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct Project {
    pub id: String,
    pub repo_path: String,
}

pub fn load_project(conn: &Connection) -> Result<Option<Project>> {
    Ok(conn
        .query_row("SELECT id, repo_path FROM project", [], |r| Ok(Project { id: r.get(0)?, repo_path: r.get(1)? }))
        .optional()?)
}

/// The project a database file belongs to, read without migrating or locking it. The backend
/// registers a project from before multi-project with it (data-model.md §10.7); another backend
/// may still have it open.
pub fn peek_project(path: &std::path::Path) -> Result<Option<Project>> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    load_project(&conn)
}

/// Records the repository the project is for. The project's id comes from its registration
/// (data-model.md §10.3).
#[derive(Debug, Serialize, Deserialize)]
pub struct Onboard {
    pub request_id: String,
    pub project_id: String,
    pub repo_path: String,
}

impl Command for Onboard {
    const NAME: &'static str = "onboard";
    type Output = ();

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_user()?;
        if let Some(project) = load_project(cx.tx)? {
            return Err(Error::rejected("already_onboarded", format!("connected to {}", project.repo_path)));
        }
        // The other columns belonged to the private store, which is gone (#86); they stay empty
        // until the schema is replaced.
        cx.tx.execute(
            "INSERT INTO project (id, repo_path, branch, author_name, author_email, integration, integration_rev,
                                  previewed, created_at)
             VALUES (?1, ?2, '', '', '', '', 0, '', ?3)",
            params![self.project_id, self.repo_path, cx.now],
        )?;
        cx.emit("project.onboarded", &self.project_id, json!({ "repo_path": self.repo_path }))?;
        Ok(())
    }
}
