//! What runs turns and keeps their transcripts: a project, or Angela (data-model.md §10.6). Both
//! have a database with the project schema, a blob store, the turns this backend runs and their
//! live items; the runner, the GUI's pushes and the transcript reads work on either.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use lobotomy_core::Db;
use lobotomy_core::item::BlobStore;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::host::Host;

/// Everything instance-scoped hangs off this value and is passed explicitly; there are no global
/// singletons, so one process runs several instances (data-model.md §10). Cross-project state is
/// in the [`Host`] it refers to.
pub struct Instance {
    /// A project's registered id (data-model.md §10.3), or [`crate::angela::ID`].
    pub id: String,
    pub data_dir: PathBuf,
    pub db: Arc<Db>,
    pub blobs: BlobStore,
    pub host: Arc<Host>,
    /// Turns this backend runs right now, with the pids to interrupt them.
    pub running: Mutex<HashMap<String, RunningTurn>>,
    /// The turn runners, and a project's store jobs: closing the instance waits for them
    /// (data-model.md §10.4).
    pub tasks: TaskTracker,
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
    _lock: InstanceLock,
}

/// Closing our descriptor alone may leave the lock held by a forked child until it execs.
/// Release it explicitly when the last owner is gone. This is the instance's last field so the
/// database is dropped before the lock is released.
struct InstanceLock(std::fs::File);

impl Drop for InstanceLock {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            tracing::warn!(%error, "could not release the instance lock");
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

#[derive(Debug)]
pub struct RunningTurn {
    pub pid: Option<u32>,
    /// Set when the runtime asked the CLI to stop; the turn then ends as interrupted.
    pub interrupt_requested: bool,
    /// Asks the runner to end and wait for its process tree before returning.
    pub stop: CancellationToken,
    pub task: Option<JoinHandle<anyhow::Result<()>>>,
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

impl Instance {
    /// Opens the instance `id` in `data_dir`, created if missing: takes the lock, then opens and
    /// migrates the database.
    pub fn open(id: &str, data_dir: &Path, host: Arc<Host>) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
        let lock = lock_file(&data_dir.join("lock"), &data_dir.display().to_string())?;
        let db = Db::open(&data_dir.join("lobotomy.db")).context("opening the database")?;
        Ok(Self {
            id: id.to_owned(),
            data_dir: data_dir.to_path_buf(),
            db: Arc::new(db),
            blobs: BlobStore::new(data_dir.join("blobs")),
            host,
            running: Mutex::new(HashMap::new()),
            tasks: TaskTracker::new(),
            live: Mutex::new(HashMap::new()),
            live_version: AtomicU64::new(0),
            wake: Notify::new(),
            _lock: InstanceLock(lock),
        })
    }

    /// Stops the runners of the turns still running: each CLI ends with its process tree, as when
    /// the backend exits. Their turns are reconciled when the instance opens next (data-model.md
    /// §3.3).
    pub async fn end_turns(&self) -> anyhow::Result<()> {
        let turns = std::mem::take(&mut *self.running.lock().unwrap());
        for turn in turns.values() {
            turn.stop.cancel();
        }
        // Join every runner, even if another failed, so no CLI is abandoned on error.
        let mut result = Ok(());
        for turn in turns.into_values() {
            if let Some(task) = turn.task {
                let ended = task.await.map_err(anyhow::Error::from).and_then(|r| r);
                if result.is_ok() {
                    result = ended;
                }
            }
        }
        result
    }

    /// Tells the GUIs about a change in this instance: `push` is a JSON object, sent with the
    /// instance's id added as `project` (frontend.md §3 rule 8).
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

    /// Raw CLI output of a turn, kept only when the turn did not end cleanly (data-model.md §7.4).
    pub fn raw_output_path(&self, turn_id: &str, ext: &str) -> PathBuf {
        self.data_dir.join("turns").join(format!("{turn_id}.{ext}"))
    }

    /// An empty directory for `GH_CONFIG_DIR`: `gh` then behaves as logged out
    /// (harness-adapter.md §1.6).
    pub fn empty_gh_config_dir(&self) -> PathBuf {
        self.data_dir.join("gh-empty")
    }

    /// The MCP URL of one turn: the instance, then the turn's token (harness-adapter.md §2).
    pub fn mcp_url(&self, token: &str) -> String {
        format!("{}/mcp/{}/{token}", self.host.mcp_base.lock().unwrap(), self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_the_instance_lock_does_not_wait_for_an_inherited_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        let file = std::fs::File::create(&path).unwrap();
        file.try_lock().unwrap();
        // A clone shares the file description, as a forked child's descriptor does on Unix.
        // No child needs to execute code or reach exec before the owner can release its lock.
        let inherited = file.try_clone().unwrap();
        let lock = InstanceLock(file);
        let next = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        assert!(matches!(next.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        drop(lock);
        next.try_lock().unwrap();
        drop(inherited);
    }
}
