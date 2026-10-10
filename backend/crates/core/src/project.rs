//! The project, its integration version and its configuration (harness-adapter.md §4.2, §4.3;
//! data-model.md §9.1, §10).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Project {
    pub id: String,
    pub repo_path: String,
    pub branch: String,
    pub author_name: String,
    pub author_email: String,
    /// The head of the integration version.
    pub integration: String,
    pub integration_rev: i64,
    /// The integration version the user's repository was last brought to.
    pub previewed: String,
}

pub fn load_project(conn: &Connection) -> Result<Option<Project>> {
    Ok(conn
        .query_row(
            "SELECT id, repo_path, branch, author_name, author_email, integration, integration_rev, previewed
             FROM project",
            [],
            |r| {
                Ok(Project {
                    id: r.get(0)?,
                    repo_path: r.get(1)?,
                    branch: r.get(2)?,
                    author_name: r.get(3)?,
                    author_email: r.get(4)?,
                    integration: r.get(5)?,
                    integration_rev: r.get(6)?,
                    previewed: r.get(7)?,
                })
            },
        )
        .optional()?)
}

pub fn require_project(conn: &Connection) -> Result<Project> {
    load_project(conn)?.ok_or_else(|| Error::rejected("not_onboarded", "no repository has been connected yet"))
}

/// The project a database file belongs to, read without migrating or locking it. The backend
/// registers a project from before multi-project with it (data-model.md §10.7); another backend
/// may still have it open.
pub fn peek_project(path: &std::path::Path) -> Result<Option<Project>> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    load_project(&conn)
}

/// A check command, run in order in the verification site; it passes with exit code 0
/// (harness-adapter.md §4.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub command: String,
    pub timeout_secs: u64,
}

/// What the project configuration holds (data-model.md §9.1 project_config). Captures, checks
/// and their results name the version they used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub checks: Vec<Check>,
    /// gitignore patterns never captured; materializing keeps them as caches.
    pub excluded: Vec<String>,
    /// Paths captured even when an ignore rule matches them.
    pub force_tracked: Vec<String>,
    /// The size guardrail on the files one capture adds (harness-adapter.md §4.1).
    pub max_new_files: usize,
    pub max_new_bytes: u64,
}

impl Default for ProjectConfig {
    /// The guardrail's starting values are placeholders until real projects are measured
    /// (harness-adapter.md §4.1).
    fn default() -> Self {
        ProjectConfig {
            checks: vec![],
            excluded: vec![],
            force_tracked: vec![],
            max_new_files: 1000,
            max_new_bytes: 50 * 1024 * 1024,
        }
    }
}

/// The current configuration and its version.
pub fn current_config(conn: &Connection) -> Result<(i64, ProjectConfig)> {
    let (version, config): (i64, String) = conn
        .query_row("SELECT version, config FROM project_config ORDER BY version DESC LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
        .ok_or_else(|| Error::rejected("not_onboarded", "no repository has been connected yet"))?;
    Ok((version, serde_json::from_str(&config)?))
}

pub fn config_version(conn: &Connection, version: i64) -> Result<ProjectConfig> {
    let config: String =
        conn.query_row("SELECT config FROM project_config WHERE version = ?1", [version], |r| r.get(0))?;
    Ok(serde_json::from_str(&config)?)
}

/// Connects the user's repository (harness-adapter.md §4.3 "接入"). The runtime has already
/// checked the repository and copied `head` into the private store; this records it as the first
/// integration version. The project's id comes from its registration (data-model.md §10.3).
#[derive(Debug, Serialize, Deserialize)]
pub struct Onboard {
    pub request_id: String,
    pub project_id: String,
    pub repo_path: String,
    pub branch: String,
    pub head: String,
    pub author_name: String,
    pub author_email: String,
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
        let id = &self.project_id;
        cx.tx.execute(
            "INSERT INTO project (id, repo_path, branch, author_name, author_email, integration, integration_rev,
                                  previewed, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?6, ?7)",
            params![id, self.repo_path, self.branch, self.author_name, self.author_email, self.head, cx.now],
        )?;
        cx.tx.execute(
            "INSERT INTO project_config (version, config, created_by, created_at) VALUES (1, ?1, ?2, ?3)",
            params![serde_json::to_string(&ProjectConfig::default())?, caller.scope(), cx.now],
        )?;
        cx.emit(
            "project.onboarded",
            id,
            json!({ "repo_path": self.repo_path, "branch": self.branch, "head": self.head }),
        )?;
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EditProjectConfig {
    pub request_id: String,
    pub expected_version: i64,
    pub config: ProjectConfig,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigEdited {
    pub version: i64,
}

impl Command for EditProjectConfig {
    const NAME: &'static str = "edit_project_config";
    type Output = ConfigEdited;

    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<ConfigEdited> {
        caller.require_user()?;
        let (current, _) = current_config(cx.tx)?;
        if current != self.expected_version {
            return Err(Error::rejected(
                "stale_version",
                format!("the configuration is at version {current}, not {}", self.expected_version),
            ));
        }
        if self.config.checks.iter().any(|c| c.command.trim().is_empty() || c.timeout_secs == 0) {
            return Err(Error::rejected("bad_check", "a check needs a command and a timeout"));
        }
        let version = current + 1;
        cx.tx.execute(
            "INSERT INTO project_config (version, config, created_by, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![version, serde_json::to_string(&self.config)?, caller.scope(), cx.now],
        )?;
        cx.emit("project.config_changed", "project", json!({ "version": version }))?;
        Ok(ConfigEdited { version })
    }
}
