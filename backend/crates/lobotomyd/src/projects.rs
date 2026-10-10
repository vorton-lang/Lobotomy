//! The projects one backend runs (data-model.md §10.3, §10.4). At startup the backend opens every
//! running project in the host's registry; the GUI adds projects while it runs. A project that
//! cannot be opened is recorded as failed, in this process only, and the others run on.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use lobotomy_core::host::{ProjectEntry, ProjectState};
use lobotomy_core::id::{new_id, now_ms};
use lobotomy_core::project::{load_project, peek_project};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::host::Host;
use crate::project::Project;
use crate::{gui, onboard, runner, scheduler};

/// One open project at work: its scheduler, and the watcher that tells GUIs what changed.
pub struct Running {
    pub project: Arc<Project>,
    scheduling: CancellationToken,
    scheduler: JoinHandle<()>,
    watching: CancellationToken,
    watcher: JoinHandle<()>,
}

impl Running {
    /// Reconciles the turns the last run left (data-model.md §3.3) and starts scheduling.
    pub async fn start(project: Arc<Project>) -> anyhow::Result<Self> {
        scheduler::recover(&project).await?;
        let watching = CancellationToken::new();
        let watcher = tokio::spawn(gui::watch(project.clone(), watching.clone()));
        let scheduling = CancellationToken::new();
        let scheduler = tokio::spawn(scheduler::run(project.clone(), scheduling.clone()));
        Ok(Self { project, scheduling, scheduler, watching, watcher })
    }

    /// Normal shutdown of this project (harness-adapter.md §1.8): its trials stop, scheduling
    /// stops, and its running CLIs are interrupted and waited for until `deadline`. The MCP
    /// service stays up meanwhile, so a CLI can still report. Store jobs are not waited for: one
    /// that has not finished runs again when the project opens next (data-model.md §5). Work
    /// still running keeps the project lock until it finishes or the process exits.
    pub async fn stop(self, deadline: Instant) {
        let _ = tokio::time::timeout_at(deadline.into(), self.project.trials.stop_all()).await;
        self.scheduling.cancel();
        let _ = self.scheduler.await;
        let running: Vec<String> = self.project.running.lock().unwrap().keys().cloned().collect();
        for turn_id in running {
            if let Err(e) = runner::interrupt(&self.project, &turn_id).await {
                tracing::warn!(project = self.project.id, turn_id, error = format!("{e:#}"), "could not interrupt");
            }
        }
        while !self.project.running.lock().unwrap().is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        self.watching.cancel();
        // Cancellation only asks the watcher to stop; join it before releasing its project.
        let _ = self.watcher.await;
    }
}

pub struct Projects {
    host: Arc<Host>,
    /// Open instances, for the GUI and the MCP service to find by id. An instance stays here
    /// until it has stopped, so its CLIs can report while they are being stopped.
    open: Mutex<BTreeMap<String, Arc<Project>>>,
    /// What runs each open instance; taken when it stops.
    runtimes: Mutex<BTreeMap<String, Running>>,
    /// Registered projects that could not be opened, with the reason (data-model.md §10.3).
    failed: Mutex<BTreeMap<String, String>>,
}

impl Projects {
    pub fn new(host: Arc<Host>) -> Self {
        Self {
            host,
            open: Mutex::new(BTreeMap::new()),
            runtimes: Mutex::new(BTreeMap::new()),
            failed: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<Project>> {
        self.open.lock().unwrap().get(id).cloned()
    }

    /// The open instances, in id order.
    pub fn all(&self) -> Vec<Arc<Project>> {
        self.open.lock().unwrap().values().cloned().collect()
    }

    /// Why a registered project is not open, if opening it failed.
    pub fn failure(&self, id: &str) -> Option<String> {
        self.failed.lock().unwrap().get(id).cloned()
    }

    /// Wakes every project's scheduler: a host change, such as a quota domain opening, may let
    /// any of them go on.
    pub fn wake_all(&self) {
        for project in self.all() {
            project.wake.notify_one();
        }
    }

    /// At startup: registers the project of a host directory from before multi-project, removes
    /// what an interrupted onboarding left, and opens every running project. Projects open side
    /// by side and do not wait for each other.
    pub async fn open_registered(&self) -> anyhow::Result<()> {
        if let Err(e) = register_legacy(&self.host) {
            tracing::warn!(error = format!("{e:#}"), "could not register the project from before multi-project");
        }
        let mut opening = Vec::new();
        for entry in self.host.db.projects()? {
            match entry.state {
                ProjectState::Onboarding => discard(&self.host, &entry).await,
                ProjectState::Running => opening.push(entry),
                ProjectState::Archived => {}
            }
        }
        let opened = futures::future::join_all(opening.iter().map(|entry| open(&self.host, entry))).await;
        for (entry, result) in opening.iter().zip(opened) {
            self.insert(entry, result);
        }
        Ok(())
    }

    fn insert(&self, entry: &ProjectEntry, result: anyhow::Result<Running>) {
        match result {
            Ok(running) => {
                self.open.lock().unwrap().insert(entry.id.clone(), running.project.clone());
                self.runtimes.lock().unwrap().insert(entry.id.clone(), running);
                self.failed.lock().unwrap().remove(&entry.id);
            }
            Err(e) => {
                let reason = format!("{e:#}");
                tracing::error!(
                    project = entry.id,
                    name = entry.name,
                    error = reason,
                    "the project could not be opened"
                );
                self.failed.lock().unwrap().insert(entry.id.clone(), reason);
            }
        }
    }

    /// A new project for the repository at `repo`, connected and running (data-model.md §10.4).
    pub async fn create(&self, repo: &Path) -> anyhow::Result<ProjectEntry> {
        let project = create(&self.host, repo, None).await?;
        let entry = self.host.db.project(&project.id)?.context("the new project is not registered")?;
        let running = Running::start(project).await;
        self.insert(&entry, running);
        self.host.changed();
        Ok(entry)
    }

    /// Stops every open project, side by side, until `deadline` (harness-adapter.md §1.8).
    pub async fn stop_all(&self, deadline: Instant) {
        let runtimes = std::mem::take(&mut *self.runtimes.lock().unwrap());
        futures::future::join_all(runtimes.into_values().map(|running| running.stop(deadline))).await;
        self.open.lock().unwrap().clear();
    }
}

/// Registers the repository at `repo` as a new project and connects it (data-model.md §10.4,
/// steps 1–3). Its data goes into `data_dir`, by default the host's directory for this project.
/// When connecting fails, neither the data directory nor the registration stays.
pub async fn create(host: &Arc<Host>, repo: &Path, data_dir: Option<PathBuf>) -> anyhow::Result<Arc<Project>> {
    let repo_path = onboard::repo_path(repo)?;
    let id = new_id("prj");
    let data_dir = data_dir.unwrap_or_else(|| host.projects_dir().join(&id));
    let name = repo_path.file_name().map_or_else(|| repo_path.display().to_string(), |n| n.to_string_lossy().into());
    let entry = ProjectEntry {
        id,
        name,
        data_dir: data_dir.to_string_lossy().into_owned(),
        repo_path: repo_path.to_string_lossy().into_owned(),
        state: ProjectState::Onboarding,
        registered_at: now_ms(),
    };
    host.db.register(&entry)?;
    let connected = async {
        let (id, dir, h) = (entry.id.clone(), data_dir.clone(), host.clone());
        let project = Arc::new(tokio::task::spawn_blocking(move || Project::open(&id, &dir, h)).await??);
        onboard::onboard(&project, &repo_path).await?;
        host.db.set_project_state(&entry.id, ProjectState::Running)?;
        anyhow::Ok(project)
    }
    .await;
    if connected.is_err() {
        discard(host, &entry).await;
    }
    connected
}

/// Opens a registered project and starts it. A lost data directory is not created again empty.
async fn open(host: &Arc<Host>, entry: &ProjectEntry) -> anyhow::Result<Running> {
    let dir = PathBuf::from(&entry.data_dir);
    if !dir.is_dir() {
        anyhow::bail!("数据目录不存在：{}", entry.data_dir);
    }
    let (id, h) = (entry.id.clone(), host.clone());
    let project = Arc::new(tokio::task::spawn_blocking(move || Project::open(&id, &dir, h)).await??);
    if project.db.read(load_project)?.is_none() {
        anyhow::bail!("数据目录中没有接入的仓库：{}", entry.data_dir);
    }
    Running::start(project).await
}

/// Deletes the data directory and the registration of an onboarding that did not finish.
async fn discard(host: &Host, entry: &ProjectEntry) {
    if let Err(e) = remove_dir(PathBuf::from(&entry.data_dir)).await {
        tracing::warn!(path = entry.data_dir, error = %e, "could not delete the data directory of a failed onboarding");
    }
    if let Err(e) = host.db.unregister_onboarding(&entry.id) {
        tracing::warn!(project = entry.id, error = %e, "could not remove the registration of a failed onboarding");
    }
}

/// Deletes a directory and what is in it. Windows may keep files open for a moment after the
/// last handle closed, so this tries again for five seconds. A missing directory is no error.
pub(crate) async fn remove_dir(root: PathBuf) -> std::io::Result<()> {
    const TRIES: u32 = 20;
    for tried in 1..=TRIES {
        let dir = root.clone();
        match tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir)).await? {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound && tried == TRIES => return Err(e),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            _ => return Ok(()),
        }
    }
    Ok(())
}

/// Before multi-project, the host directory's `gui.json` named the one project. The first start
/// after the upgrade registers it, its data directory where it is (data-model.md §10.7). A
/// project that was never connected is left alone.
fn register_legacy(host: &Host) -> anyhow::Result<()> {
    if !host.db.projects()?.is_empty() {
        return Ok(());
    }
    let Ok(text) = std::fs::read_to_string(host.dir.join("gui.json")) else {
        return Ok(());
    };
    let config: serde_json::Value = serde_json::from_str(&text).context("reading gui.json")?;
    let Some(dir) = config["project"].as_str().map(PathBuf::from) else {
        return Ok(());
    };
    let database = dir.join("lobotomy.db");
    if !database.exists() {
        return Ok(());
    }
    let Some(found) = peek_project(&database)? else {
        return Ok(());
    };
    let name = Path::new(&found.repo_path)
        .file_name()
        .map_or_else(|| found.repo_path.clone(), |n| n.to_string_lossy().into_owned());
    host.db.register(&ProjectEntry {
        id: found.id.clone(),
        name,
        data_dir: dir.to_string_lossy().into_owned(),
        repo_path: found.repo_path,
        state: ProjectState::Running,
        registered_at: now_ms(),
    })?;
    tracing::info!(project = found.id, data_dir = %dir.display(), "registered the project from before multi-project");
    Ok(())
}
