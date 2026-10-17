//! The worker host: what its task instances share (worker-interface.md §1), and what it gives the
//! agent driver.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Context;
use lobotomy_agent::{AgentHost, HarnessConfig, Instance};
use lobotomy_core::role::Role;
use lobotomy_core::turn::Failure;
use lobotomy_harness::{Harness, Permission};
use lobotomy_worker_protocol::event::{Event, Quota, Resources};
use tokio::sync::broadcast;

use crate::store::Store;

const INSTRUCTIONS: &str = include_str!("prompts/worker.md");

/// How the worker host runs its agent: one harness, and the model and effort its tasks get.
#[derive(Clone, Debug)]
pub struct Config {
    pub harness: Harness,
    pub cli: HarnessConfig,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission: Permission,
}

/// What goes to the subscriber, once stored.
#[derive(Clone, Debug)]
pub enum Outgoing {
    Event(Event),
    Resources(Resources),
}

pub struct WorkerHost {
    /// Where it keeps `worker.db` and a directory per task.
    pub root: PathBuf,
    pub config: Config,
    pub store: Store,
    /// The agent's MCP service, such as `http://127.0.0.1:4100`. Set once the server listens.
    pub mcp_base: Mutex<String>,
    pub outgoing: broadcast::Sender<Outgoing>,
    /// Each harness's quota as the last turn on it saw it.
    quota: Mutex<BTreeMap<String, Quota>>,
}

impl WorkerHost {
    pub fn open(root: PathBuf, config: Config) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
        let store = Store::open(&root.join("worker.db"))?;
        Ok(Self {
            root,
            config,
            store,
            mcp_base: Mutex::new(String::new()),
            outgoing: broadcast::channel(1024).0,
            quota: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn task_dir(&self, task_id: &str) -> PathBuf {
        self.root.join("tasks").join(task_id)
    }

    pub fn resources(&self) -> Resources {
        Resources { quota: self.quota.lock().unwrap().values().cloned().collect() }
    }

    /// A turn on `harness` completed: its quota is open again, if it was not.
    pub fn quota_passed(&self, harness: &str) {
        let changed = self.quota.lock().unwrap().get_mut(harness).is_some_and(|q| {
            let was = q.blocked;
            *q = Quota { harness: harness.to_owned(), blocked: false, resets_at: None, message: None };
            was
        });
        if changed {
            let _ = self.outgoing.send(Outgoing::Resources(self.resources()));
        }
    }
}

impl AgentHost for WorkerHost {
    fn harness(&self) -> &HarnessConfig {
        &self.config.cli
    }

    fn permission(&self, _harness: Harness) -> anyhow::Result<Permission> {
        Ok(self.config.permission)
    }

    /// Never: the worker host reports a refused quota and starts nothing on its own. The
    /// orchestrator decides when to go on (worker-interface.md §3).
    fn is_blocked(&self, _harness: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn quota_rejected(&self, harness: &str, failure: &Failure) -> anyhow::Result<()> {
        tracing::warn!(harness, message = failure.message, "quota refused");
        let quota = Quota {
            harness: harness.to_owned(),
            blocked: true,
            resets_at: failure.resets_at,
            message: Some(failure.message.clone()),
        };
        self.quota.lock().unwrap().insert(harness.to_owned(), quota);
        let _ = self.outgoing.send(Outgoing::Resources(self.resources()));
        Ok(())
    }

    fn mcp_base(&self) -> String {
        self.mcp_base.lock().unwrap().clone()
    }

    fn instructions(&self, role: &Role) -> String {
        INSTRUCTIONS.replace("{name}", &role.name)
    }

    /// The task's own directory. Until the environment has a repository (tech-plan.md §7 step 1,
    /// group 5), it starts empty.
    fn working_dir(&self, instance: &Instance<Self>, _role: &Role) -> anyhow::Result<PathBuf> {
        let dir = instance.data_dir.join("work");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(dir)
    }
}
