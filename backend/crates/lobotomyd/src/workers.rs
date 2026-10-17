//! The workers the backend gives tasks to (tech-plan.md §7 step 1, group 7; worker-interface.md):
//! one worker host process for each entry of `<host>/workers.toml`, spoken to over the worker
//! protocol on its stdin and stdout. Each entry names the project it works for. The worker's
//! events go into that project's database (`lobotomy_core::worker`); a worker host that exits is
//! started again after a while, and resumes from the events the project has.
//!
//! ```toml
//! [worker.lobotomy-codex]
//! project = "Lobotomy"
//! agent = { harness = "codex", model = "gpt-6.1-sol", effort = "medium" }
//! source = { forge = "github", repo = "vorton-lang/Lobotomy", app_id = 5333181, app_key = "…/key.pem" }
//! author = { name = "lobotomy-worker[bot]", email = "…@users.noreply.github.com" }
//! evidence = { required = ["ci-ok"] }
//! budget = { turns = 40, ci_rounds = 3 }
//! authorization = { paths = ["notes/", "AGENTS.md"] }
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{Context, bail};
use lobotomy_agent::runner::{db, runtime};
use lobotomy_core::Caller;
use lobotomy_core::role::SyncWorkerRoles;
use lobotomy_core::task::{Action, Task, criteria_text, load_task};
use lobotomy_core::worker::{
    ApplyWorkerEvent, ApplyWorkerSnapshot, CloseWorkerTask, MessageForwarded, WorkerTask, WorkerTaskCreated,
    WorkerTaskRevised, worker_seqs, worker_task,
};
use lobotomy_worker_protocol::event::{AttemptState, EVENT, Event, RESOURCES, Resources};
use lobotomy_worker_protocol::ops::{self, Budget, CreateTaskParams, Hello, HelloParams, Subscribe, SubscribeParams};
use lobotomy_worker_protocol::rpc::{self, Id, Message, Request, Response};
use lobotomy_worker_protocol::{Method, PROTOCOL_VERSION};
use serde::Deserialize;
use tokio::io::BufReader;
use tokio::process::{ChildStdin, Command};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::host::Host;
use crate::project::Project;
use crate::projects::Projects;

/// How long a stopped worker host waits before it is started again.
const RESTART: Duration = Duration::from_secs(5);
/// A call that has no answer by then fails; the worker host may be stuck.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    worker: BTreeMap<String, Entry>,
}

/// One worker of `workers.toml`.
#[derive(Clone, Debug, Deserialize)]
pub struct Entry {
    /// The name of the project it works for.
    pub project: String,
    pub agent: Agent,
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub author: Option<Author>,
    #[serde(default)]
    pub evidence: EvidenceRule,
    #[serde(default)]
    pub budget: Budget,
    #[serde(default)]
    pub authorization: Authorization,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Agent {
    pub harness: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission: Option<String>,
    /// Variables the worker host, and so its agent's CLI, gets, such as another provider's
    /// endpoint (tech-plan.md §4.5).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// The repository the worker's tasks work on, and its forge.
#[derive(Clone, Debug, Deserialize)]
pub struct Source {
    pub forge: Option<String>,
    /// `owner/name` on the forge.
    pub repo: Option<String>,
    /// Where to clone from, when not the forge's repository, such as a local path.
    pub upstream: Option<String>,
    pub base: Option<String>,
    pub app_id: Option<u64>,
    pub app_key: Option<String>,
    /// The forge's API, when not GitHub's.
    pub api: Option<String>,
    pub co_author: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Author {
    pub name: String,
    pub email: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EvidenceRule {
    /// The checks a pull request's version must pass (worker-interface.md §6).
    #[serde(default)]
    pub required: Vec<String>,
}

/// What adopting needs the user's authorization for (tech-plan.md §4.6).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Authorization {
    /// Paths of the repository, prefixes of the changed paths, such as `notes/`.
    #[serde(default)]
    pub paths: Vec<String>,
}

impl Entry {
    /// Whether its tasks go out as pull requests.
    pub fn pull_requests(&self) -> bool {
        self.source.as_ref().is_some_and(|s| s.forge.is_some())
    }

    /// The protected paths among `files`.
    pub fn needs_authorization<'a>(&self, files: &'a [String]) -> Vec<&'a str> {
        let paths = &self.authorization.paths;
        files.iter().filter(|f| paths.iter().any(|p| f.starts_with(p.as_str()))).map(String::as_str).collect()
    }

    fn args(&self, root: &Path) -> Vec<String> {
        let mut args =
            vec!["--root".into(), root.to_string_lossy().into_owned(), "--harness".into(), self.agent.harness.clone()];
        let mut flag = |name: &str, value: &Option<String>| {
            if let Some(value) = value {
                args.push(format!("--{name}"));
                args.push(value.clone());
            }
        };
        flag("model", &self.agent.model);
        flag("effort", &self.agent.effort);
        flag("permission", &self.agent.permission);
        if let Some(source) = &self.source {
            flag("upstream", &source.upstream);
            flag("base", &source.base);
            flag("forge", &source.forge);
            flag("repo", &source.repo);
            flag("forge-api", &source.api);
            flag("co-author", &source.co_author);
            flag("app-id", &source.app_id.map(|id| id.to_string()));
            flag("app-key", &source.app_key);
        }
        if let Some(author) = &self.author {
            flag("author-name", &Some(author.name.clone()));
            flag("author-email", &Some(author.email.clone()));
        }
        args
    }
}

/// A running worker host: its stdin, and the calls waiting for an answer.
struct Client {
    /// Gone once the backend quits: the worker host sees its stdin close.
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<i64, oneshot::Sender<Response>>>,
    next: AtomicI64,
}

impl Client {
    async fn call<M: Method>(&self, params: &M::Params) -> anyhow::Result<M::Result> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let request = Message::Request(Request::new::<M>(Id::Number(id), params));
        let mut stdin = self.stdin.lock().await;
        let stdin = stdin.as_mut().context("the backend is quitting")?;
        rpc::write(stdin, &request).await.context("writing to the worker host")?;
        let response = tokio::time::timeout(CALL_TIMEOUT, rx)
            .await
            .with_context(|| format!("{} got no answer", M::NAME))?
            .context("the worker host went away")?;
        Ok(response.into_result::<M>()?)
    }
}

pub struct Workers {
    host: Arc<Host>,
    projects: Weak<Projects>,
    entries: BTreeMap<String, Entry>,
    clients: Mutex<HashMap<String, Arc<Client>>>,
    stop: CancellationToken,
    tasks: TaskTracker,
}

impl Workers {
    /// Reads `<host>/workers.toml`; without it there are no workers.
    pub fn load(host: Arc<Host>, projects: &Arc<Projects>) -> anyhow::Result<Arc<Self>> {
        let path = host.dir.join("workers.toml");
        let file: File = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Arc::new(Self {
            host,
            projects: Arc::downgrade(projects),
            entries: file.worker,
            clients: Mutex::new(HashMap::new()),
            stop: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }))
    }

    /// Starts a worker host for each entry, kept running until [`Workers::shutdown`].
    pub fn start(self: &Arc<Self>) {
        for name in self.entries.keys() {
            self.tasks.spawn(self.clone().supervise(name.clone()));
        }
    }

    /// Closes the worker hosts' stdin, which tells them to quit, and waits for them until
    /// `deadline`. Those left end when this process does.
    pub async fn shutdown(&self, deadline: std::time::Instant) {
        self.stop.cancel();
        let clients: Vec<Arc<Client>> = self.clients.lock().unwrap().drain().map(|(_, c)| c).collect();
        for client in clients {
            client.stdin.lock().await.take();
        }
        self.tasks.close();
        if tokio::time::timeout_at(deadline.into(), self.tasks.wait()).await.is_err() {
            tracing::warn!("worker hosts still running at the end of the grace period");
        }
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.get(name)
    }

    /// The project's workers: each one's name and harness.
    pub fn of_project(&self, project: &str) -> Vec<(String, String)> {
        self.entries
            .iter()
            .filter(|(_, e)| e.project == project)
            .map(|(n, e)| (n.clone(), e.agent.harness.clone()))
            .collect()
    }

    /// Calls the worker host; fails while it is not running.
    pub async fn call<M: Method>(&self, worker: &str, params: &M::Params) -> anyhow::Result<M::Result> {
        let client = self.clients.lock().unwrap().get(worker).cloned();
        let client = client.with_context(|| format!("worker {worker} is not running"))?;
        client.call::<M>(params).await
    }

    /// Gives the project's workers their queued tasks (worker-interface.md §3 `create_task`).
    pub async fn dispatch(&self, project: &Arc<Project>) -> anyhow::Result<()> {
        let name = self.project_name(&project.id)?;
        let workers = self.of_project(&name);
        let queued = db(project, move |db| {
            db.read(|c| {
                let mut out = Vec::new();
                for (worker, _) in &workers {
                    if let Some(task) = lobotomy_core::task::next_queued_task(c, worker)? {
                        let criteria = criteria_text(c, &task.id, task.criteria_version)?;
                        out.push((worker.clone(), task, criteria));
                    }
                }
                Ok(out)
            })
        })
        .await?;
        for (worker, task, criteria) in queued {
            let Some(entry) = self.entry(&worker) else { continue };
            let params = CreateTaskParams {
                task_id: task.id.clone(),
                spec_version: task.criteria_version as u32,
                spec: spec(entry, &task, &criteria),
            };
            let created = match self.call::<ops::CreateTask>(&worker, &params).await {
                Ok(created) => created,
                Err(e) => {
                    tracing::warn!(
                        worker,
                        task = task.id,
                        error = format!("{e:#}"),
                        "could not give the task to its worker"
                    );
                    continue;
                }
            };
            let recorded = WorkerTaskCreated {
                task_id: task.id.clone(),
                worker: worker.clone(),
                spec_version: task.criteria_version,
                attempt_id: created.attempt_id,
            };
            lobotomy_agent::ignore_rejection(runtime(project, recorded).await)?;
        }
        Ok(())
    }

    /// The project's roles for its workers, as the configuration lists them.
    pub async fn sync_roles(&self, project: &Project) -> anyhow::Result<()> {
        let workers = self.of_project(&self.project_name(&project.id)?);
        runtime(project, SyncWorkerRoles { workers }).await
    }

    fn project_name(&self, id: &str) -> anyhow::Result<String> {
        Ok(self.host.db.project(id)?.with_context(|| format!("no project {id}"))?.name)
    }

    /// The open project of this name.
    fn project(&self, name: &str) -> Option<Arc<Project>> {
        let projects = self.projects.upgrade()?;
        let entry = self.host.db.projects().ok()?.into_iter().find(|p| p.name == name)?;
        projects.get(&entry.id)
    }

    async fn supervise(self: Arc<Self>, name: String) {
        loop {
            let ran = self.run_once(&name).await;
            self.clients.lock().unwrap().remove(&name);
            if self.stop.is_cancelled() {
                return;
            }
            match ran {
                Ok(()) => tracing::warn!(worker = name, "the worker host exited"),
                Err(e) => tracing::error!(worker = name, error = format!("{e:#}"), "the worker host failed"),
            }
            tokio::select! {
                _ = tokio::time::sleep(RESTART) => {}
                _ = self.stop.cancelled() => return,
            }
        }
    }

    /// Runs the worker host until it exits: says hello, resumes from what the project has, and
    /// applies its events as they come.
    async fn run_once(&self, name: &str) -> anyhow::Result<()> {
        let entry = self.entry(name).context("no such worker")?.clone();
        let project = self.project(&entry.project).with_context(|| format!("project {} is not open", entry.project))?;
        let root = self.host.dir.join("workers").join(name);
        let mut command = Command::new(worker_program()?);
        command
            .args(entry.args(&root))
            .envs(&entry.agent.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = command.spawn().context("starting the worker host")?;
        let client = Arc::new(Client {
            stdin: tokio::sync::Mutex::new(Some(child.stdin.take().context("no stdin")?)),
            pending: Mutex::new(HashMap::new()),
            next: AtomicI64::new(1),
        });
        let stdout = BufReader::new(child.stdout.take().context("no stdout")?);
        let reader = tokio::spawn(read(stdout, client.clone(), project.clone(), name.to_owned()));

        let hello = client.call::<Hello>(&HelloParams { protocols: vec![PROTOCOL_VERSION.into()] }).await?;
        tracing::info!(worker = name, protocol = hello.protocol, "worker host ready");
        let worker = name.to_owned();
        let after: BTreeMap<String, u64> = db(&project, move |db| db.read(|c| worker_seqs(c, &worker)))
            .await?
            .into_iter()
            .map(|(t, s)| (t, s as u64))
            .collect();
        let snapshot = client.call::<Subscribe>(&SubscribeParams { after }).await?;
        for task in snapshot.tasks {
            runtime(&project, ApplyWorkerSnapshot { snapshot: task }).await?;
        }
        project.wake.notify_one();
        self.clients.lock().unwrap().insert(name.to_owned(), client);
        let read = reader.await;
        let _ = child.wait().await;
        read?
    }
}

/// Reads the worker host's stdout: answers go to their calls, events into the project.
async fn read(
    mut stdout: BufReader<tokio::process::ChildStdout>,
    client: Arc<Client>,
    project: Arc<Project>,
    worker: String,
) -> anyhow::Result<()> {
    loop {
        let message = match rpc::read(&mut stdout).await {
            Ok(Some(message)) => message,
            Ok(None) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                tracing::warn!(worker, error = %e, "ignored a line from the worker host");
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        match message {
            Message::Response(response) => {
                let id = match &response.id {
                    Id::Number(n) => *n,
                    Id::String(_) => continue,
                };
                if let Some(tx) = client.pending.lock().unwrap().remove(&id) {
                    let _ = tx.send(response);
                }
            }
            Message::Notification(n) if n.method == EVENT => {
                let event: Event = match serde_json::from_value(n.params) {
                    Ok(event) => event,
                    Err(e) => {
                        tracing::warn!(worker, error = %e, "ignored an event that does not parse");
                        continue;
                    }
                };
                let apply = ApplyWorkerEvent { task_id: event.task_id.clone(), event };
                match lobotomy_agent::ignore_rejection(runtime(&project, apply).await) {
                    Ok(Some(true)) => project.wake.notify_one(),
                    Ok(_) => {}
                    Err(e) => tracing::error!(worker, error = format!("{e:#}"), "could not apply the worker's event"),
                }
            }
            Message::Notification(n) if n.method == RESOURCES => {
                if let Ok(resources) = serde_json::from_value::<Resources>(n.params) {
                    for quota in resources.quota.iter().filter(|q| q.blocked) {
                        tracing::warn!(
                            worker,
                            harness = quota.harness,
                            message = quota.message,
                            "the worker's quota is refused"
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// The worker host program: `LOBOTOMY_WORKER` if set, else `lobotomy-worker` next to this backend.
fn worker_program() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("LOBOTOMY_WORKER") {
        return Ok(path.into());
    }
    let exe = std::env::current_exe().context("locating the backend executable")?;
    let program = exe.with_file_name(format!("lobotomy-worker{}", std::env::consts::EXE_SUFFIX));
    if !program.exists() {
        bail!("no worker host at {}", program.display());
    }
    Ok(program)
}

/// A task as its worker gets it (tech-plan.md §2.2): the user's words, the manager's reading, what
/// it may do, when it is done and when to stop.
fn spec(entry: &Entry, task: &Task, criteria: &str) -> ops::TaskSpec {
    let assignment = task.assignment.as_ref();
    let mut actions: Vec<ops::Action> = match assignment {
        Some(a) => a
            .actions
            .iter()
            .map(|a| match a {
                Action::ReadOnly => ops::Action::ReadOnly,
                Action::EditDocs => ops::Action::EditDocs,
                Action::EditCode => ops::Action::EditCode,
            })
            .collect(),
        None => vec![ops::Action::EditCode],
    };
    let edits = actions.iter().any(|a| matches!(a, ops::Action::EditCode | ops::Action::EditDocs));
    let pull_request = edits && entry.pull_requests();
    if pull_request {
        actions.push(ops::Action::OpenPullRequest);
    }
    ops::TaskSpec {
        title: task.title.clone(),
        user_words: task.body.clone(),
        interpretation: assignment.map(|a| a.interpretation.clone()).unwrap_or_default(),
        actions,
        done_when: criteria.to_owned(),
        required_checks: if pull_request { entry.evidence.required.clone() } else { Vec::new() },
        stop_when: assignment.map(|a| a.stop_when.clone()).unwrap_or_default(),
        budget: entry.budget.clone(),
        ask_when: "遇到只有用户能决定的问题时".into(),
    }
}

/// The task, for the tools that act on a worker's task.
pub async fn task(project: &Project, task_id: &str) -> anyhow::Result<Task> {
    let id = task_id.to_owned();
    db(project, move |db| db.read(|c| load_task(c, &id))).await
}

/// Who stands behind adopting a change that touches the paths needing the user's authorization
/// (tech-plan.md §4.6).
pub enum Authority {
    /// The user, in the GUI.
    User,
    /// The user's message the manager names, checked to be the user's.
    Message(String),
    /// Nobody: such a change is refused.
    None,
}

/// What the worker said of a task, if a worker runs it (tech-plan.md §7 step 1).
pub async fn worker_of(project: &Project, task_id: &str) -> anyhow::Result<Option<WorkerTask>> {
    let id = task_id.to_owned();
    db(project, move |db| db.read(|c| worker_task(c, &id))).await
}

impl Workers {
    /// Sends the project's message `message_id` to the task's worker (worker-interface.md §3
    /// `send_message`); it answers the worker's open question, if there is one. The message stays
    /// in the project as the record, out of the role's inbox.
    pub async fn forward(
        &self,
        project: &Project,
        worker: &WorkerTask,
        task_id: &str,
        message_id: &str,
    ) -> anyhow::Result<()> {
        let id = message_id.to_owned();
        let body: String = db(project, move |db| {
            db.read(|c| Ok(c.query_row("SELECT body FROM message WHERE id = ?1", [&id], |r| r.get(0))?))
        })
        .await?;
        let params = ops::SendMessageParams {
            task_id: task_id.to_owned(),
            message_id: message_id.to_owned(),
            body,
            refs: Vec::new(),
            question_id: worker.status.question.as_ref().map(|q| q.question_id.clone()),
        };
        self.call::<ops::SendMessage>(&worker.worker, &params).await?;
        runtime(project, MessageForwarded { message_id: message_id.to_owned() }).await
    }

    /// Accepts a worker's delivery: its change is merged at the version read, where it touches
    /// protected paths only with the user's authority, and the task closes.
    pub async fn adopt(
        &self,
        project: &Project,
        caller: Caller,
        worker: &WorkerTask,
        task_id: &str,
        criteria_version: i64,
        authority: Authority,
    ) -> anyhow::Result<String> {
        let task = task(project, task_id).await?;
        if task.criteria_version != criteria_version {
            return Err(lobotomy_core::Error::rejected(
                "stale_criteria",
                format!("criteria are at version {}, not {criteria_version}", task.criteria_version),
            )
            .into());
        }
        if !matches!(worker.status.state, AttemptState::Delivered | AttemptState::Adopted) {
            bail!("worker 还没有交付（{:?}）", worker.status.state);
        }
        let entry = self.entry(&worker.worker).context("这个 worker 不在配置里")?;
        let mut merged = None;
        if let Some(change) = worker.change() {
            let protected = entry.needs_authorization(&change.files);
            if !protected.is_empty() && matches!(authority, Authority::None) {
                bail!(
                    "改动碰到需要用户授权的路径：{}。先问用户；用户同意后，把他那条消息的 ID 填进 authorization",
                    protected.join("、")
                );
            }
            let params = ops::AdoptParams {
                task_id: task_id.to_owned(),
                artifact_id: change.artifact_id.clone(),
                version: change.version.clone(),
            };
            match self.call::<ops::Adopt>(&worker.worker, &params).await? {
                ops::AdoptOutcome::Adopted => merged = Some(change.version.clone()),
                ops::AdoptOutcome::Rejected { reason, detail } => bail!("没有合并（{reason:?}）：{detail}"),
            }
        }
        let close = ops::CloseParams { task_id: task_id.to_owned(), outcome: ops::CloseOutcome::Adopted };
        self.call::<ops::Close>(&worker.worker, &close).await?;
        let reason = match authority {
            Authority::Message(message) => format!("用户授权：{message}"),
            _ => String::new(),
        };
        let cmd = CloseWorkerTask {
            request_id: lobotomy_core::id::new_id("req"),
            task_id: task_id.to_owned(),
            outcome: ops::CloseOutcome::Adopted,
            reason,
        };
        db(project, move |db| db.execute(&caller, &cmd)).await?;
        Ok(match merged {
            Some(version) => format!("已合并（{}）并关闭任务。", &version[..version.len().min(10)]),
            None => "已采用交付并关闭任务。".to_owned(),
        })
    }

    /// Abandons a worker's task: the worker stops, its pull request closes, and the task closes.
    pub async fn abandon(
        &self,
        project: &Project,
        caller: Caller,
        worker: &WorkerTask,
        task_id: &str,
        reason: String,
    ) -> anyhow::Result<()> {
        let close = ops::CloseParams { task_id: task_id.to_owned(), outcome: ops::CloseOutcome::Abandoned };
        self.call::<ops::Close>(&worker.worker, &close).await?;
        let cmd = CloseWorkerTask {
            request_id: lobotomy_core::id::new_id("req"),
            task_id: task_id.to_owned(),
            outcome: ops::CloseOutcome::Abandoned,
            reason,
        };
        db(project, move |db| db.execute(&caller, &cmd)).await?;
        Ok(())
    }

    /// Interrupts the worker's running turn; the attempt then waits for a message.
    pub async fn interrupt(&self, worker: &WorkerTask, task_id: &str) -> anyhow::Result<()> {
        let stop = ops::StopParams { task_id: task_id.to_owned(), level: ops::StopLevel::Turn };
        self.call::<ops::Stop>(&worker.worker, &stop).await?;
        Ok(())
    }

    /// The task's criteria changed: the worker starts a new attempt on the new spec
    /// (worker-interface.md §3 `revise`).
    pub async fn revise(&self, project: &Project, worker: &WorkerTask, task_id: &str) -> anyhow::Result<()> {
        let entry = self.entry(&worker.worker).context("这个 worker 不在配置里")?;
        let id = task_id.to_owned();
        let (task, criteria) = db(project, move |db| {
            db.read(|c| {
                let task = load_task(c, &id)?;
                let criteria = criteria_text(c, &id, task.criteria_version)?;
                Ok((task, criteria))
            })
        })
        .await?;
        let params = CreateTaskParams {
            task_id: task.id.clone(),
            spec_version: task.criteria_version as u32,
            spec: spec(entry, &task, &criteria),
        };
        let revised = self.call::<ops::Revise>(&worker.worker, &params).await?;
        let cmd = WorkerTaskRevised {
            task_id: task.id.clone(),
            spec_version: task.criteria_version,
            attempt_id: revised.attempt_id,
        };
        runtime(project, cmd).await
    }
}
