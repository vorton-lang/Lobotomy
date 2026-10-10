use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use lobotomy_core::Db;
use lobotomy_core::item::BlobStore;
use lobotomy_store::Store;
use tokio::sync::Notify;

use crate::host::Host;

/// One project instance. Everything project-scoped hangs off this value and is passed explicitly;
/// there are no global singletons, so one process runs several instances (data-model.md §10).
/// Cross-project state is in the [`Host`] it refers to.
pub struct Project {
    /// The id it is registered under, the same as its project row's (data-model.md §10.3).
    pub id: String,
    pub data_dir: PathBuf,
    pub trials: crate::trial::Trials,
    pub db: Arc<Db>,
    pub blobs: BlobStore,
    /// The private store of results (harness-adapter.md §4).
    pub store: Arc<Store>,
    pub host: Arc<Host>,
    /// Turns this backend runs right now, with the pids to interrupt them.
    pub running: Mutex<HashMap<String, RunningTurn>>,
    /// Store work in progress (materializing, capturing, verifying, previewing), by key, so the
    /// scheduler starts each job once.
    pub jobs: Mutex<HashSet<String>>,
    /// Store work that failed, by key, with the reason. It waits for the user's retry.
    pub failed: Mutex<HashMap<String, String>>,
    /// The verification site is one directory: a verification holds this from writing it until
    /// the processes of its last check have ended (#12).
    pub verify_site: tokio::sync::Mutex<()>,
    /// Items of running turns that have started and not completed, for live display. Kept in
    /// memory only; a backend crash loses them (frontend.md §3).
    pub live: Mutex<HashMap<String, LiveTurn>>,
    /// Bumped on every change to `live`, so the GUI service knows when to push it.
    pub live_version: AtomicU64,
    /// Wakes the scheduler after a change.
    pub wake: Notify,
    /// An exclusive lock on `<data_dir>/lock`, held while the instance lives: two backends on one
    /// data directory would run two schedulers on one database (#16). The OS releases it when
    /// the process ends, however it ends.
    _lock: ProjectLock,
}

/// Closing our descriptor alone may leave the lock held by a forked child until it execs.
/// Release it explicitly when the last project owner is gone. This is the project's last
/// field so the database and store fields are dropped before the lock is released.
struct ProjectLock(std::fs::File);

impl Drop for ProjectLock {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            tracing::warn!(%error, "could not release the project lock");
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct LiveTurn {
    pub role: String,
    pub started_at: i64,
    /// By the harness's item id.
    pub items: BTreeMap<String, LiveItem>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct LiveItem {
    pub kind: String,
    pub content: serde_json::Value,
    pub started_at: i64,
}

#[derive(Clone, Debug)]
pub struct RunningTurn {
    pub pid: Option<u32>,
    /// Set when the runtime asked the CLI to stop; the turn then ends as interrupted.
    pub interrupt_requested: bool,
}

/// Takes an exclusive lock on `path`, created if missing. `what` names the holder in the refusal.
pub fn lock_file(path: &Path, what: &str) -> anyhow::Result<std::fs::File> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(std::fs::TryLockError::WouldBlock) => anyhow::bail!("another backend is already running on {what}"),
        Err(std::fs::TryLockError::Error(e)) => Err(e).with_context(|| format!("locking {}", path.display())),
    }
}

impl Project {
    /// Opens the project `id` in `data_dir`, created if missing. A data directory that belongs to
    /// another project is refused.
    pub fn open(id: &str, data_dir: &Path, host: Arc<Host>) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
        let lock = lock_file(&data_dir.join("lock"), &data_dir.display().to_string())?;
        let db = Db::open(&data_dir.join("lobotomy.db")).context("opening the project database")?;
        if let Some(found) = db.read(lobotomy_core::project::load_project)?
            && found.id != id
        {
            anyhow::bail!("{} holds project {}, not {id}", data_dir.display(), found.id);
        }
        let store = Store::open_or_init(&data_dir.join("store")).context("opening the private store")?;
        Ok(Self {
            id: id.to_owned(),
            data_dir: data_dir.to_path_buf(),
            trials: crate::trial::Trials::default(),
            db: Arc::new(db),
            blobs: BlobStore::new(data_dir.join("blobs")),
            store: Arc::new(store),
            host,
            running: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashSet::new()),
            failed: Mutex::new(HashMap::new()),
            verify_site: tokio::sync::Mutex::new(()),
            live: Mutex::new(HashMap::new()),
            live_version: AtomicU64::new(0),
            wake: Notify::new(),
            _lock: ProjectLock(lock),
        })
    }

    /// Tells the GUIs about a change in this project: `push` is a JSON object, sent with the
    /// project's id added (frontend.md §3 rule 8).
    pub fn push(&self, mut push: serde_json::Value) {
        push["project"] = self.id.clone().into();
        let _ = self.host.gui_push.send(push.to_string().into());
    }

    /// Changes the live view of running turns.
    pub fn update_live(&self, change: impl FnOnce(&mut HashMap<String, LiveTurn>)) {
        change(&mut self.live.lock().unwrap());
        self.live_version.fetch_add(1, Ordering::Relaxed);
    }

    /// A slot's directory: fixed per slot, outside the user's repository (harness-adapter.md §3).
    pub fn slot_dir(&self, slot: &str) -> PathBuf {
        self.data_dir.join("slots").join(slot)
    }

    /// Where jj keeps its state for one generation of a slot, out of the agent's reach.
    pub fn slot_state_dir(&self, slot: &str, generation: i64) -> PathBuf {
        self.data_dir.join("workspaces").join(slot).join(format!("g{generation}"))
    }

    /// The fixed verification site (data-model.md §5) and its jj state.
    pub fn verify_dir(&self) -> PathBuf {
        self.data_dir.join("verify")
    }

    pub fn verify_state_dir(&self) -> PathBuf {
        self.data_dir.join("workspaces").join("verify")
    }

    /// Directories moved aside, deleted once their replacement is ready (data-model.md §6).
    pub fn quarantine_dir(&self) -> PathBuf {
        self.data_dir.join("quarantine")
    }

    /// Candidate copies of user-started trials, one directory per trial. A trial deletes its copy
    /// when it ends; startup deletes what is left.
    pub fn trials_dir(&self) -> PathBuf {
        self.data_dir.join("trials")
    }

    /// Raw CLI output of a turn, kept only when the turn did not end cleanly (data-model.md §7.4).
    pub fn raw_output_path(&self, turn_id: &str, ext: &str) -> PathBuf {
        self.data_dir.join("turns").join(format!("{turn_id}.{ext}"))
    }

    /// An empty directory for `GH_CONFIG_DIR`: `gh` then behaves as logged out
    /// (harness-adapter.md §1.6).
    pub fn empty_gh_config_dir(&self) -> PathBuf {
        self.data_dir.join("gh-empty")
    }

    /// The MCP URL of one turn: the project, then the turn's token (harness-adapter.md §2).
    pub fn mcp_url(&self, token: &str) -> String {
        format!("{}/mcp/{}/{token}", self.host.mcp_base.lock().unwrap(), self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_the_project_lock_does_not_wait_for_an_inherited_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        let file = std::fs::File::create(&path).unwrap();
        file.try_lock().unwrap();
        // A clone shares the file description, as a forked child's descriptor does on Unix.
        // No child needs to execute code or reach exec before the owner can release its lock.
        let inherited = file.try_clone().unwrap();
        let lock = ProjectLock(file);
        let next = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        assert!(matches!(next.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        drop(lock);
        next.try_lock().unwrap();
        drop(inherited);
    }
}
