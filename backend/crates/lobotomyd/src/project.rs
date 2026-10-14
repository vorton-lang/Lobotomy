use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use lobotomy_store::Store;
use tokio::task::AbortHandle;

use crate::host::Host;
use crate::instance::Instance;

/// One project instance: what every instance has (its [`Instance`], which it derefs to), and what
/// only a project has: the private store of results, the store jobs and the trials.
pub struct Project {
    pub trials: crate::trial::Trials,
    /// The private store of results (harness-adapter.md §4).
    pub store: Arc<Store>,
    /// Store work in progress (materializing, capturing, verifying, previewing), by key, so the
    /// scheduler starts each job once; with the handle that stops it.
    pub jobs: Mutex<HashMap<String, AbortHandle>>,
    /// Store work that failed, by key, with the reason. It waits for the user's retry.
    pub failed: Mutex<HashMap<String, String>>,
    /// Serializes observation/adoption admission with starts and job scheduling (#67).
    pub activity: tokio::sync::Mutex<()>,
    /// The verification site is one directory: a verification holds this from writing it until
    /// the processes of its last check have ended (#12).
    pub verify_site: tokio::sync::Mutex<()>,
    /// The last field, so the store is dropped before the instance releases its lock.
    pub instance: Arc<Instance>,
}

impl Deref for Project {
    type Target = Instance;

    fn deref(&self) -> &Instance {
        &self.instance
    }
}

impl Project {
    /// Opens the project `id` in `data_dir`, created if missing. A data directory that belongs to
    /// another project is refused.
    pub fn open(id: &str, data_dir: &Path, host: Arc<Host>) -> anyhow::Result<Self> {
        let instance = Instance::open(id, data_dir, host)?;
        if let Some(found) = instance.db.read(lobotomy_core::project::load_project)?
            && found.id != id
        {
            anyhow::bail!("{} holds project {}, not {id}", data_dir.display(), found.id);
        }
        let store = Store::open_or_init(&data_dir.join("store")).context("opening the private store")?;
        Ok(Self {
            trials: crate::trial::Trials::default(),
            store: Arc::new(store),
            jobs: Mutex::new(HashMap::new()),
            failed: Mutex::new(HashMap::new()),
            activity: tokio::sync::Mutex::new(()),
            verify_site: tokio::sync::Mutex::new(()),
            instance: Arc::new(instance),
        })
    }

    /// Stops the running verifications: their check commands end with everything they started.
    /// A verification stopped so has no result; it runs again when the project opens next
    /// (data-model.md §5).
    pub fn stop_checks(&self) {
        for (key, job) in self.jobs.lock().unwrap().iter() {
            if key.starts_with("verify:") || key.starts_with("adopt:") {
                job.abort();
            }
        }
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
}
