use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use lobotomy_core::Db;
use lobotomy_core::item::BlobStore;
use tokio::sync::Notify;

/// One project instance. Everything project-scoped hangs off this value and is passed explicitly;
/// there are no global singletons, so several instances can share one process later
/// (data-model.md §10).
pub struct Project {
    pub data_dir: PathBuf,
    pub db: Arc<Db>,
    pub blobs: BlobStore,
    /// Where harness CLIs run and how they are started.
    pub harness: HarnessConfig,
    /// The MCP service's base URL, such as `http://127.0.0.1:4100`. Set once the server listens.
    pub mcp_base: Mutex<String>,
    /// Turns this backend runs right now, with the pids to interrupt them.
    pub running: Mutex<HashMap<String, RunningTurn>>,
    /// Wakes the scheduler after a change.
    pub wake: Notify,
}

#[derive(Clone, Debug)]
pub struct RunningTurn {
    pub pid: Option<u32>,
    /// Set when the runtime asked the CLI to stop; the turn then ends as interrupted.
    pub interrupt_requested: bool,
}

/// How to start a harness CLI.
#[derive(Clone, Debug)]
pub struct HarnessConfig {
    /// The Codex program and any arguments that go before the adapter's own. Tests put a fake
    /// CLI here.
    pub codex: Vec<String>,
    /// The program and arguments that send Ctrl+C to a pid (harness-adapter.md §1.8).
    pub interrupt_helper: Vec<String>,
    pub codex_reasoning_effort: Option<String>,
}

impl HarnessConfig {
    /// The real CLIs, with this executable as the interrupt helper.
    pub fn detect() -> anyhow::Result<Self> {
        let exe = std::env::current_exe().context("locating the backend executable")?;
        Ok(Self {
            codex: vec![lobotomy_harness::codex::locate().to_string_lossy().into_owned()],
            interrupt_helper: vec![exe.to_string_lossy().into_owned(), "ctrl-c".into()],
            codex_reasoning_effort: None,
        })
    }
}

impl Project {
    pub fn open(data_dir: &Path, harness: HarnessConfig) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
        let db = Db::open(&data_dir.join("lobotomy.db")).context("opening the project database")?;
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            db: Arc::new(db),
            blobs: BlobStore::new(data_dir.join("blobs")),
            harness,
            mcp_base: Mutex::new(String::new()),
            running: Mutex::new(HashMap::new()),
            wake: Notify::new(),
        })
    }

    /// The role's execution site. Increment 3 materializes slots from the private store; until
    /// then it is a plain directory (harness-adapter.md §3).
    pub fn slot_dir(&self, slot: &str) -> PathBuf {
        self.data_dir.join("slots").join(slot)
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
