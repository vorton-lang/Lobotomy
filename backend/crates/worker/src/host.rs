//! The worker host: what its task instances share (worker-interface.md §1), and what it gives the
//! agent driver.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Context;
use lobotomy_agent::{AgentHost, HarnessConfig, Instance};
use lobotomy_core::role::Role;
use lobotomy_core::turn::{Failure, Turn};
use lobotomy_harness::{Harness, Permission};
use lobotomy_worker_protocol::event::{Event, Quota, Resources};
use tokio::sync::broadcast;

use crate::environment::{AttemptDirs, Environment, Repo};
use crate::forge::{self, GitHub};
use crate::store::{self, AttemptRow, Store};

const INSTRUCTIONS: &str = include_str!("prompts/worker.md");

/// How the worker host runs its agent: one harness, and the model and effort its tasks get.
#[derive(Clone, Debug)]
pub struct Config {
    pub harness: Harness,
    pub cli: HarnessConfig,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission: Permission,
    /// The repository tasks work on. Without one, each task works in a plain folder.
    pub repo: Option<Repo>,
    /// The forge the repository is on, for pull requests, checks and merges.
    pub forge: Option<forge::Config>,
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
    pub environment: Option<Environment>,
    pub forge: Option<GitHub>,
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
        let environment = config.repo.clone().map(|repo| Environment::new(&root, repo));
        let forge = config.forge.clone().map(GitHub::new).transpose()?;
        Ok(Self {
            root,
            config,
            store,
            environment,
            forge,
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

    /// The clone of the turn's attempt; without a repository, a folder of the task's own.
    fn working_dir(&self, instance: &Instance<Self>, turn: &Turn, _role: &Role) -> anyhow::Result<PathBuf> {
        if self.environment.is_none() {
            let dir = instance.data_dir.join("work");
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            return Ok(dir);
        }
        let attempt = self.turn_attempt(&instance.id, turn)?;
        Ok(AttemptDirs::new(&instance.data_dir, attempt.seq).clone)
    }

    fn env(&self, instance: &Instance<Self>, turn: &Turn) -> anyhow::Result<Vec<(String, String)>> {
        let Some(environment) = &self.environment else { return Ok(Vec::new()) };
        let attempt = self.turn_attempt(&instance.id, turn)?;
        let port_base = attempt.port_base.with_context(|| format!("attempt {} is not prepared", attempt.id))?;
        Ok(environment.agent_env(&AttemptDirs::new(&instance.data_dir, attempt.seq), port_base))
    }
}

impl WorkerHost {
    /// The variables that give git the forge's token; none without a forge.
    pub async fn forge_git_env(&self) -> anyhow::Result<Vec<(String, String)>> {
        match &self.forge {
            Some(forge) => forge.git_env().await,
            None => Ok(Vec::new()),
        }
    }

    /// The attempt a turn of the task works on.
    fn turn_attempt(&self, task_id: &str, turn: &Turn) -> anyhow::Result<AttemptRow> {
        let core_task = turn.task_id.as_deref().with_context(|| format!("turn {} belongs to no attempt", turn.id))?;
        self.store.read(|c| store::attempt_by_core(c, task_id, core_task))
    }
}
