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
/// there are no global singletons, so several instances can share one process later
/// (data-model.md §10). Cross-project state is in the [`Host`] it refers to.
pub struct Project {
    pub data_dir: PathBuf,
    pub db: Arc<Db>,
    pub blobs: BlobStore,
    /// The private store of results (harness-adapter.md §4).
    pub store: Arc<Store>,
    pub host: Arc<Host>,
    /// The MCP service's base URL, such as `http://127.0.0.1:4100`. Set once the server listens.
    pub mcp_base: Mutex<String>,
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
    /// The GUI asked the backend to stop (frontend.md §1).
    pub shutdown_requested: Notify,
    /// What connected GUIs are told (see `gui`), as JSON text.
    pub gui_push: tokio::sync::broadcast::Sender<Arc<str>>,
    /// An exclusive lock on `<data_dir>/lock`, held while the instance lives: two backends on one
    /// data directory would run two schedulers on one database (#16). The OS releases it when
    /// the process ends, however it ends.
    _lock: std::fs::File,
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

impl Project {
    pub fn open(data_dir: &Path, host: Arc<Host>) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
        let lock_path = data_dir.join("lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("opening {}", lock_path.display()))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                anyhow::bail!("another backend is already running on {}", data_dir.display())
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("locking {}", lock_path.display()));
            }
        }
        let db = Db::open(&data_dir.join("lobotomy.db")).context("opening the project database")?;
        let store = Store::open_or_init(&data_dir.join("store")).context("opening the private store")?;
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            db: Arc::new(db),
            blobs: BlobStore::new(data_dir.join("blobs")),
            store: Arc::new(store),
            host,
            mcp_base: Mutex::new(String::new()),
            running: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashSet::new()),
            failed: Mutex::new(HashMap::new()),
            verify_site: tokio::sync::Mutex::new(()),
            live: Mutex::new(HashMap::new()),
            live_version: AtomicU64::new(0),
            wake: Notify::new(),
            shutdown_requested: Notify::new(),
            gui_push: tokio::sync::broadcast::channel(crate::gui::PUSH_BUFFER).0,
            _lock: lock,
        })
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

    /// Raw CLI output of a turn, kept only when the turn did not end cleanly (data-model.md §7.4).
    pub fn raw_output_path(&self, turn_id: &str, ext: &str) -> PathBuf {
        self.data_dir.join("turns").join(format!("{turn_id}.{ext}"))
    }

    /// An empty directory for `GH_CONFIG_DIR`: `gh` then behaves as logged out
    /// (harness-adapter.md §1.6).
    pub fn empty_gh_config_dir(&self) -> PathBuf {
        self.data_dir.join("gh-empty")
    }

    pub fn mcp_url(&self, token: &str) -> String {
        format!("{}/mcp/{token}", self.mcp_base.lock().unwrap())
    }
}
