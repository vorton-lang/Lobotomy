//! The host: what all projects of this user share, outside any project (data-model.md §10):
//! the project registry, the quota domains, the harness configuration, the listening address and
//! the GUI connections.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use lobotomy_agent::AgentHost;
pub use lobotomy_agent::harness::{Cli, HarnessConfig};
use lobotomy_core::Error;
use lobotomy_core::host::HostDb;
use lobotomy_core::id::now_ms;
use lobotomy_core::quota::{CheckOutcome, Domain};
use lobotomy_core::role::Role;
use lobotomy_core::turn::{Failure, Turn};
use lobotomy_harness::{Harness, Model, Permission, claude};
use tokio::sync::{Notify, broadcast};

use crate::instance::Instance;

const WORKER_INSTRUCTIONS: &str = include_str!("prompts/worker.md");
const MANAGER_INSTRUCTIONS: &str = include_str!("prompts/manager.md");

pub struct Host {
    pub dir: PathBuf,
    pub db: HostDb,
    pub harness: HarnessConfig,
    /// What a GUI connection must present (frontend.md §1). Generated once, kept in
    /// `<dir>/gui-token`; never put into the environment of roles or checks.
    pub gui_token: String,
    /// The MCP service's base URL, such as `http://127.0.0.1:4100`. Set once the server listens.
    pub mcp_base: Mutex<String>,
    /// What connected GUIs are told (see `gui`), as JSON text. Pushes about a project name it.
    pub gui_push: broadcast::Sender<Arc<str>>,
    /// The GUI asked the backend to stop (frontend.md §1).
    pub shutdown_requested: Notify,
    /// The workers tasks go to (tech-plan.md §7 step 1), once the backend has read their
    /// configuration.
    pub workers: std::sync::OnceLock<Arc<crate::workers::Workers>>,
    /// Domains with a check in progress: at most one check per domain (data-model.md §8.4).
    checking: Mutex<HashSet<String>>,
    /// Codex's model catalog, once read; [`Host::models`] starts reading it.
    codex_models: Mutex<Vec<Model>>,
    catalog_requested: AtomicBool,
}

/// Marks a domain as being checked, and clears the mark however the check ends: done, failed,
/// timed out or cancelled.
struct CheckingMark<'a> {
    checking: &'a Mutex<HashSet<String>>,
    harness: String,
}

impl<'a> CheckingMark<'a> {
    fn acquire(checking: &'a Mutex<HashSet<String>>, harness: &str) -> Option<Self> {
        checking.lock().unwrap().insert(harness.to_owned()).then(|| Self { checking, harness: harness.to_owned() })
    }
}

impl Drop for CheckingMark<'_> {
    fn drop(&mut self) {
        self.checking.lock().unwrap().remove(&self.harness);
    }
}

impl Host {
    pub fn open(dir: &Path, harness: HarnessConfig) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let db = HostDb::open(&dir.join("host.db")).context("opening the host database")?;
        let gui_token = gui_token(&dir.join("gui-token"))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            db,
            harness,
            gui_token,
            mcp_base: Mutex::new(String::new()),
            gui_push: broadcast::channel(crate::gui::PUSH_BUFFER).0,
            shutdown_requested: Notify::new(),
            workers: std::sync::OnceLock::new(),
            checking: Mutex::new(HashSet::new()),
            codex_models: Mutex::new(Vec::new()),
            catalog_requested: AtomicBool::new(false),
        })
    }

    /// The models the user can choose for a role on a harness (harness-adapter.md §1.2). Codex's
    /// come from its catalog, read once, the first time they are asked for; until it is read, or
    /// when it cannot be, there are none.
    pub fn models(self: &Arc<Self>, harness: Harness) -> Vec<Model> {
        match harness {
            Harness::Claude => claude::models(),
            Harness::Codex => {
                if !self.catalog_requested.swap(true, Ordering::Relaxed) {
                    let host = self.clone();
                    tokio::spawn(async move { host.load_models().await });
                }
                self.codex_models.lock().unwrap().clone()
            }
        }
    }

    /// Reads Codex's model catalog and tells the GUIs. It calls no model; without it, the GUI
    /// takes a model's name as the user types it.
    async fn load_models(&self) {
        match tokio::time::timeout(Duration::from_secs(30), self.harness.codex_catalog(&self.dir)).await {
            Ok(Ok(models)) => {
                *self.codex_models.lock().unwrap() = models;
                self.changed();
            }
            Ok(Err(e)) => tracing::warn!(error = format!("{e:#}"), "Codex's model catalog could not be read"),
            Err(_) => tracing::warn!("reading Codex's model catalog timed out"),
        }
    }

    /// Where new projects keep their data, one directory per project id (data-model.md §10.3).
    pub fn projects_dir(&self) -> PathBuf {
        self.dir.join("projects")
    }

    /// Tells the GUIs to take a new host snapshot: a host setting or the project list changed.
    /// Neither has an event of its own.
    pub fn changed(&self) {
        let _ = self.gui_push.send(serde_json::json!(crate::gui::protocol::Push::Host).to_string().into());
    }

    /// `%LOCALAPPDATA%\Lobotomy` on Windows; `$XDG_STATE_HOME/lobotomy` or
    /// `~/.local/state/lobotomy` elsewhere.
    pub fn default_dir() -> anyhow::Result<PathBuf> {
        if cfg!(windows) {
            let base = std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?;
            return Ok(PathBuf::from(base).join("Lobotomy"));
        }
        if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
            return Ok(PathBuf::from(state).join("lobotomy"));
        }
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        Ok(PathBuf::from(home).join(".local").join("state").join("lobotomy"))
    }

    pub fn is_blocked(&self, harness: &str) -> anyhow::Result<bool> {
        Ok(self.db.domain(harness)?.is_blocked())
    }

    /// The harness's permission mode (harness-adapter.md §1.9): full access until the user
    /// changes it. Each turn reads it, so a change applies from the next turn on.
    pub fn permission(&self, harness: Harness) -> anyhow::Result<Permission> {
        match self.db.permission(harness.as_str())? {
            None => Ok(Permission::default()),
            Some(value) => Permission::parse(&value).ok_or_else(|| {
                Error::invariant(format!(
                    "unknown permission mode {value:?} for {} in the host database",
                    harness.as_str()
                ))
                .into()
            }),
        }
    }

    /// The user's choice; only the GUI makes it.
    pub fn set_permission(&self, harness: &str, permission: &str) -> anyhow::Result<Permission> {
        let Some(harness) = Harness::parse(harness) else {
            return Err(Error::rejected("unknown_harness", format!("unknown harness {harness}")).into());
        };
        let Some(parsed) = Permission::parse(permission) else {
            return Err(Error::rejected("unknown_permission", format!("unknown permission mode {permission}")).into());
        };
        self.db.set_permission(harness.as_str(), parsed.as_str(), lobotomy_core::id::now_ms())?;
        tracing::info!(harness = harness.as_str(), permission, "permission mode changed");
        Ok(parsed)
    }

    /// The user's retry: one quota check for the domain, with the result recorded
    /// (data-model.md §8.4). The only way a blocked domain opens again. A check already in
    /// progress is not repeated; the current state is returned instead.
    pub async fn retry(self: &Arc<Self>, harness: &str) -> anyhow::Result<Domain> {
        let Some(_mark) = CheckingMark::acquire(&self.checking, harness) else {
            return Ok(self.db.domain(harness)?);
        };
        let outcome = self.probe(harness).await.unwrap_or_else(|e| {
            tracing::warn!(harness, error = format!("{e:#}"), "quota check could not run");
            CheckOutcome::Rejected { resets_at: None }
        });
        tracing::info!(harness, ?outcome, "quota check");
        Ok(self.db.record_check(harness, &outcome)?)
    }

    async fn probe(&self, name: &str) -> anyhow::Result<CheckOutcome> {
        let harness = Harness::parse(name).with_context(|| format!("no quota check for {name}"))?;
        let permission = self.permission(harness)?;
        self.harness.probe(harness, permission, &self.dir).await
    }
}

impl AgentHost for Host {
    fn harness(&self) -> &HarnessConfig {
        &self.harness
    }

    fn permission(&self, harness: Harness) -> anyhow::Result<Permission> {
        Host::permission(self, harness)
    }

    fn is_blocked(&self, harness: &str) -> anyhow::Result<bool> {
        Host::is_blocked(self, harness)
    }

    fn quota_rejected(&self, harness: &str, failure: &Failure) -> anyhow::Result<()> {
        if self.db.block(harness, failure.resets_at, &failure.message, now_ms())? {
            tracing::warn!(harness, message = failure.message, "quota domain blocked");
        }
        Ok(())
    }

    fn mcp_base(&self) -> String {
        self.mcp_base.lock().unwrap().clone()
    }

    /// By the role's kind.
    fn instructions(&self, role: &Role) -> String {
        let text = if role.kind == "manager" { MANAGER_INSTRUCTIONS } else { WORKER_INSTRUCTIONS };
        text.replace("{name}", &role.name)
    }

    /// An empty directory of the role's own. The manager's is her instance's `work`
    /// (data-model.md §10.6); a project's roles each have one under theirs. Roles get no copy of
    /// the repository until the worker layer is replaced (#86).
    fn working_dir(&self, instance: &Instance, _turn: &Turn, role: &Role) -> anyhow::Result<PathBuf> {
        let work = instance.data_dir.join("work");
        let dir = if role.kind == "manager" { work } else { work.join(&role.name) };
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(dir)
    }
}

/// The GUI token, created on first use: two ULIDs give 160 random bits.
fn gui_token(path: &Path) -> anyhow::Result<String> {
    if let Ok(token) = std::fs::read_to_string(path)
        && !token.trim().is_empty()
    {
        return Ok(token.trim().to_owned());
    }
    let token = format!("{}{}", ulid_part(), ulid_part());
    std::fs::write(path, &token).with_context(|| format!("writing {}", path.display()))?;
    Ok(token)
}

fn ulid_part() -> String {
    let id = lobotomy_core::id::new_id("t");
    id.trim_start_matches("t_").to_lowercase()
}
