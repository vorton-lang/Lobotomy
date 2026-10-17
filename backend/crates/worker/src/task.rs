//! The worker host's tasks (worker-interface.md §2–§5). Each task has an instance of the agent
//! driver: its own database with the project schema, where the agent is the one role and each
//! attempt is a task of that database. The turn, message and question rules there are the
//! backend's (data-model.md §3, §4); this module runs them and says what they mean in the
//! protocol.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lobotomy_agent::ignore_rejection;
use lobotomy_agent::instance::Instance;
use lobotomy_agent::runner::{self, db, launch, launch_registered, reconcile, runtime};
use lobotomy_core::Caller;
use lobotomy_core::question::open_questions;
use lobotomy_core::task::{
    Abandon, CreateTask, Phase, SendBack, SendMessage, StartAttempt, load_task, occupant, queued_messages,
};
use lobotomy_core::turn::{
    Continue, EndTurn, Hold, NewNativeSession, Outcome, RegisterTurn, TurnState, hold, last_turn, unfinished_turn,
};
use lobotomy_worker_protocol::event::{AttemptState, Blocked, Event, EventBody, Need, Question, Receipt};
use lobotomy_worker_protocol::ops::{
    Action, AttemptRef, CloseOutcome, CloseParams, CreateTaskParams, Revised, SendMessageParams, StopLevel, StopParams,
    Stopped, TaskSpec,
};
use lobotomy_worker_protocol::rpc::RpcError;
use rusqlite::{Connection, OptionalExtension, Transaction};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::host::{Outgoing, WorkerHost};
use crate::store::{self, AttemptRow, MessageRow, NewTask, Status, TaskRow};

/// The agent's role in each task's instance.
pub const ROLE: &str = "Worker";

/// How often a task looks again without being woken, mainly to see whether the CLI of an unknown
/// turn has exited.
const POLL: Duration = Duration::from_secs(1);

pub type TaskInstance = Instance<WorkerHost>;

type OpResult<T> = Result<T, RpcError>;

/// A refusal as the protocol says it; anything else is an internal error.
pub fn rpc_error(e: anyhow::Error) -> RpcError {
    if let Some(e) = e.downcast_ref::<RpcError>() {
        return e.clone();
    }
    match e.downcast_ref::<lobotomy_core::Error>() {
        Some(lobotomy_core::Error::Rejected { code, message }) => RpcError::rejected(code, message.clone()),
        _ => RpcError::new(lobotomy_worker_protocol::rpc::INTERNAL_ERROR, format!("{e:#}")),
    }
}

fn rejected(reason: &str, message: impl Into<String>) -> RpcError {
    RpcError::rejected(reason, message)
}

/// A task at work: its instance and the loop that runs it.
struct Open {
    instance: Arc<TaskInstance>,
    stop: CancellationToken,
    handle: JoinHandle<()>,
}

pub struct Tasks {
    pub host: Arc<WorkerHost>,
    open: Mutex<HashMap<String, Open>>,
}

impl Tasks {
    pub fn new(host: Arc<WorkerHost>) -> Arc<Self> {
        Arc::new(Self { host, open: Mutex::new(HashMap::new()) })
    }

    /// The instance of an open task, for its agent's MCP calls.
    pub fn instance(&self, task_id: &str) -> Option<Arc<TaskInstance>> {
        self.open.lock().unwrap().get(task_id).map(|o| o.instance.clone())
    }

    /// Opens every task that still has something to do. Its turns left unfinished by the last run
    /// are unknown until their CLI has been seen to exit (worker-interface.md §9).
    pub async fn recover(self: &Arc<Self>) -> anyhow::Result<()> {
        let host = self.host.clone();
        let tasks = tokio::task::spawn_blocking(move || host.store.read(store::tasks)).await??;
        for task in tasks {
            let finished = task.closed.is_some() && task.status.state.is_final();
            if !finished && let Err(e) = self.ensure_open(&task.id).await {
                tracing::error!(task = task.id, error = format!("{e:#}"), "could not open the task");
            }
        }
        Ok(())
    }

    /// Stops every task's loop, then interrupts their running CLIs and waits for them until
    /// `deadline`, as quitting does (harness-adapter.md §1.8).
    pub async fn shutdown(&self, deadline: Instant) {
        let open: Vec<Open> = self.open.lock().unwrap().drain().map(|(_, o)| o).collect();
        for o in &open {
            o.stop.cancel();
        }
        for o in open {
            let _ = o.handle.await;
            runner::interrupt_all(&o.instance, deadline).await;
        }
    }

    async fn ensure_open(self: &Arc<Self>, task_id: &str) -> anyhow::Result<Arc<TaskInstance>> {
        if let Some(instance) = self.instance(task_id) {
            return Ok(instance);
        }
        let (host, id) = (self.host.clone(), task_id.to_owned());
        let instance = Arc::new(
            tokio::task::spawn_blocking(move || {
                let instance = Instance::open(&id, &host.task_dir(&id), host.clone())?;
                let config = &host.config;
                let (harness, model, effort) = (config.harness.as_str(), config.model.clone(), config.effort.clone());
                instance.db.write(|tx| seed(tx, harness, model.as_deref(), effort.as_deref()))?;
                anyhow::Ok(instance)
            })
            .await??,
        );
        runner::recover(&instance).await?;
        let mut open = self.open.lock().unwrap();
        if let Some(existing) = open.get(task_id) {
            return Ok(existing.instance.clone());
        }
        let stop = CancellationToken::new();
        let handle = tokio::spawn(run(self.clone(), task_id.to_owned(), instance.clone(), stop.clone()));
        open.insert(task_id.to_owned(), Open { instance: instance.clone(), stop, handle });
        Ok(instance)
    }

    fn wake(&self, task_id: &str) {
        if let Some(instance) = self.instance(task_id) {
            instance.wake.notify_one();
        }
    }

    async fn write<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let host = self.host.clone();
        tokio::task::spawn_blocking(move || host.store.write(f)).await?
    }

    async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let host = self.host.clone();
        tokio::task::spawn_blocking(move || host.store.read(f)).await?
    }

    /// Sends stored events to the subscriber, if any.
    fn publish(&self, events: Vec<Event>) {
        for event in events {
            let _ = self.host.outgoing.send(Outgoing::Event(event));
        }
    }

    async fn task_row(&self, task_id: &str) -> OpResult<TaskRow> {
        let id = task_id.to_owned();
        self.read(move |c| store::task(c, &id))
            .await
            .map_err(rpc_error)?
            .ok_or_else(|| rejected("unknown_task", format!("no task {task_id}")))
    }

    // ---- operations (worker-interface.md §3) ----

    pub async fn create_task(self: &Arc<Self>, params: CreateTaskParams) -> OpResult<AttemptRef> {
        let request = serde_json::to_string(&params).expect("params are JSON serializable");
        let id = params.task_id.clone();
        if let Some(task) = self.read(move |c| store::task(c, &id)).await.map_err(rpc_error)? {
            return if task.request == request {
                let first = task.id.clone();
                let attempt = self
                    .read(move |c| {
                        Ok(c.query_row("SELECT id FROM attempt WHERE task_id = ?1 AND seq = 1", [&first], |r| {
                            r.get(0)
                        })?)
                    })
                    .await
                    .map_err(rpc_error)?;
                Ok(AttemptRef { attempt_id: attempt })
            } else {
                Err(rejected("idempotency_key_reused", format!("task {} exists with other params", task.id)))
            };
        }
        self.check_spec(&params.spec)?;
        let instance = self.ensure_open(&params.task_id).await.map_err(rpc_error)?;
        let attempt_id = lobotomy_core::id::new_id("attempt");
        let core_task = start_core_task(&instance, &attempt_id, &params.spec).await.map_err(rpc_error)?;
        let (task_id, spec_version, spec, attempt) =
            (params.task_id, params.spec_version, params.spec, attempt_id.clone());
        let events = self
            .write(move |tx| {
                let new = NewTask {
                    id: &task_id,
                    request: &request,
                    spec_version,
                    spec: &spec,
                    attempt_id: &attempt,
                    core_task: &core_task,
                };
                store::insert_task(tx, &new)?;
                Ok(vec![store::append(tx, &task_id, &attempt, Status::of(AttemptState::Submitted).event())?])
            })
            .await
            .map_err(rpc_error)?;
        self.publish(events);
        instance.wake.notify_one();
        Ok(AttemptRef { attempt_id })
    }

    /// What this worker host cannot do is refused before anything starts (§3).
    fn check_spec(&self, spec: &TaskSpec) -> OpResult<()> {
        if spec.title.trim().is_empty() {
            return Err(rejected("invalid_spec", "a task needs a title"));
        }
        if spec.actions.is_empty() {
            return Err(rejected("invalid_spec", "a task names the actions it allows"));
        }
        if let Some(check) = spec.required_checks.first() {
            return Err(rejected("unsupported", format!("this environment provides no evidence, so not {check}")));
        }
        if spec.actions.contains(&Action::OpenPullRequest) {
            return Err(rejected("unsupported", "this environment opens no pull requests"));
        }
        Ok(())
    }

    pub async fn send_message(self: &Arc<Self>, params: SendMessageParams) -> OpResult<AttemptRef> {
        let request = serde_json::to_string(&params).expect("params are JSON serializable");
        let id = params.message_id.clone();
        if let Some(message) = self.read(move |c| store::message(c, &id)).await.map_err(rpc_error)? {
            return if message.request == request {
                Ok(AttemptRef { attempt_id: message.attempt_id })
            } else {
                Err(rejected("idempotency_key_reused", format!("message {} exists with other params", message.id)))
            };
        }
        let task = self.task_row(&params.task_id).await?;
        let attempt = self.attempt(&task.attempt_id).await?;
        if task.closed.is_some() || attempt.ended.is_some() || attempt.ending.is_some() {
            return Err(rejected("attempt_ended", format!("attempt {} takes no more messages", attempt.id)));
        }
        let instance = self.ensure_open(&task.id).await.map_err(rpc_error)?;
        let core_id = deliver(&instance, &attempt, &params).await.map_err(rpc_error)?;
        let (row_task, row_attempt) = (task.id.clone(), attempt.id.clone());
        let row = MessageRow {
            id: params.message_id,
            task_id: row_task.clone(),
            attempt_id: row_attempt.clone(),
            request,
            core_id,
            receipt: Receipt::Queued,
        };
        let events = self
            .write(move |tx| {
                store::insert_message(tx, &row)?;
                let body = EventBody::MessageReceipt { message_id: row.id.clone(), receipt: Receipt::Queued };
                Ok(vec![store::append(tx, &row_task, &row_attempt, body)?])
            })
            .await
            .map_err(rpc_error)?;
        self.publish(events);
        instance.wake.notify_one();
        Ok(AttemptRef { attempt_id: attempt.id })
    }

    pub async fn revise(self: &Arc<Self>, params: CreateTaskParams) -> OpResult<Revised> {
        let (id, version) = (params.task_id.clone(), params.spec_version);
        if let Some(existing) = self.read(move |c| store::attempt_for_spec(c, &id, version)).await.map_err(rpc_error)? {
            return Ok(Revised { attempt_id: existing.id, superseded: existing.superseded });
        }
        let task = self.task_row(&params.task_id).await?;
        if task.closed.is_some() {
            return Err(rejected("task_closed", format!("task {} is closed", task.id)));
        }
        if params.spec_version <= task.spec_version {
            return Err(rejected("stale_spec", format!("task {} is at spec version {}", task.id, task.spec_version)));
        }
        self.check_spec(&params.spec)?;
        let instance = self.ensure_open(&task.id).await.map_err(rpc_error)?;
        let old = self.attempt(&task.attempt_id).await?;
        // Messages the old attempt has not taken in go back to the orchestrator (§3).
        let messages = {
            let id = old.id.clone();
            self.read(move |c| store::attempt_messages(c, &id)).await.map_err(rpc_error)?
        };
        let mut superseded = Vec::new();
        for m in messages {
            let core_id = m.core_id.clone();
            let queued: bool = db(&instance, move |db| {
                db.read(|c| {
                    Ok(c.query_row("SELECT state = 'queued' FROM message WHERE id = ?1", [&core_id], |r| r.get(0))?)
                })
            })
            .await
            .map_err(rpc_error)?;
            if queued {
                superseded.push(m.id);
            }
        }
        let attempt_id = lobotomy_core::id::new_id("attempt");
        let core_task = start_core_task(&instance, &attempt_id, &params.spec).await.map_err(rpc_error)?;
        let (task_id, attempt, dropped) = (task.id.clone(), attempt_id.clone(), superseded.clone());
        let events = self
            .write(move |tx| {
                let mut events = Vec::new();
                for message_id in &dropped {
                    let receipt = Receipt::NotDelivered { reason: "superseded".into() };
                    let body = EventBody::MessageReceipt { message_id: message_id.clone(), receipt };
                    events.push(store::append(tx, &task_id, &old.id, body)?);
                }
                store::set_ending(tx, &old.id, AttemptState::Canceled)?;
                store::insert_attempt(tx, &task_id, &attempt, params.spec_version, &core_task, &dropped)?;
                store::set_current(tx, &task_id, &attempt, params.spec_version, &params.spec)?;
                events.push(store::append(tx, &task_id, &attempt, Status::of(AttemptState::Submitted).event())?);
                Ok(events)
            })
            .await
            .map_err(rpc_error)?;
        self.publish(events);
        instance.wake.notify_one();
        Ok(Revised { attempt_id, superseded })
    }

    pub async fn stop(self: &Arc<Self>, params: StopParams) -> OpResult<Stopped> {
        let task = self.task_row(&params.task_id).await?;
        let attempt = self.attempt(&task.attempt_id).await?;
        if attempt.ended.is_none() {
            match params.level {
                StopLevel::Turn => {
                    if let Some(instance) = self.instance(&task.id) {
                        interrupt_running(&instance);
                    }
                }
                StopLevel::Attempt => {
                    let id = attempt.id.clone();
                    self.write(move |tx| store::set_ending(tx, &id, AttemptState::Canceled))
                        .await
                        .map_err(rpc_error)?;
                    self.wake(&task.id);
                }
            }
        }
        Ok(Stopped { attempt_id: attempt.id, state: task.status.state })
    }

    pub async fn close(self: &Arc<Self>, params: CloseParams) -> OpResult<()> {
        let task = self.task_row(&params.task_id).await?;
        match task.closed {
            Some(outcome) if outcome == params.outcome => return Ok(()),
            Some(outcome) => {
                return Err(rejected(
                    "task_closed",
                    format!("task {} is closed as {}", task.id, store::close_text(outcome)),
                ));
            }
            None => {}
        }
        let attempt = self.attempt(&task.attempt_id).await?;
        if params.outcome == CloseOutcome::Adopted && task.status.state != AttemptState::Adopted {
            return Err(rejected("not_adopted", format!("attempt {} has not been adopted", attempt.id)));
        }
        let (task_id, attempt_id) = (task.id.clone(), attempt.id.clone());
        self.write(move |tx| {
            if attempt.ended.is_none() {
                store::set_ending(tx, &attempt_id, AttemptState::Abandoned)?;
            }
            store::set_closed(tx, &task_id, params.outcome)
        })
        .await
        .map_err(rpc_error)?;
        self.wake(&task.id);
        Ok(())
    }

    async fn attempt(&self, attempt_id: &str) -> OpResult<AttemptRow> {
        let id = attempt_id.to_owned();
        self.read(move |c| store::attempt(c, &id)).await.map_err(rpc_error)
    }
}

/// The instance's database keeps only the agent's role, added once. Its migrations add the
/// projects' roles; they go while nothing refers to them.
fn seed(tx: &Transaction<'_>, harness: &str, model: Option<&str>, effort: Option<&str>) -> lobotomy_core::Result<()> {
    tx.execute(
        "DELETE FROM role WHERE name <> ?1
           AND name NOT IN (SELECT role FROM native_session) AND name NOT IN (SELECT role FROM message)
           AND name NOT IN (SELECT executor FROM task) AND name NOT IN (SELECT role FROM thread)",
        [ROLE],
    )?;
    tx.execute(
        "INSERT INTO role (name, kind, harness) SELECT ?1, 'worker', ?2 WHERE NOT EXISTS (SELECT 1 FROM role WHERE name = ?1)",
        [ROLE, harness],
    )?;
    if model.is_some() || effort.is_some() {
        tx.execute(
            "INSERT INTO role_model (role, harness, model, effort) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (role, harness) DO UPDATE SET model = excluded.model, effort = excluded.effort",
            rusqlite::params![ROLE, harness, model, effort],
        )?;
    }
    Ok(())
}

/// The attempt's task in the instance: the spec as the agent reads it, and `done_when` as its
/// criteria. It starts once the agent's role is free.
async fn start_core_task(instance: &TaskInstance, attempt_id: &str, spec: &TaskSpec) -> anyhow::Result<String> {
    let create = CreateTask {
        request_id: attempt_id.to_owned(),
        title: spec.title.clone(),
        body: spec_text(spec),
        criteria: spec.done_when.clone(),
        executor: ROLE.into(),
    };
    Ok(db(instance, move |db| db.execute(&Caller::User, &create)).await?.id)
}

/// The spec as the agent reads it, under the brief's own header.
fn spec_text(spec: &TaskSpec) -> String {
    let actions: Vec<&str> = spec
        .actions
        .iter()
        .map(|a| match a {
            Action::ReadOnly => "只读调查",
            Action::EditDocs => "改文档",
            Action::EditCode => "改代码",
            Action::OpenPullRequest => "开 PR",
        })
        .collect();
    let mut parts = vec![
        format!("【用户原话】\n{}", spec.user_words),
        format!("【解读】\n{}", spec.interpretation),
        format!("允许的动作：{}", actions.join("、")),
    ];
    for (title, text) in [("什么时候停下来汇报", &spec.stop_when), ("什么时候停下来问", &spec.ask_when)]
    {
        if !text.trim().is_empty() {
            parts.push(format!("{title}：{text}"));
        }
    }
    parts.join("\n\n")
}

/// Puts the orchestrator's message in the agent's inbox, by the attempt's state: review rework
/// after a delivery, a continuation after a turn that did not end normally, else the next turn's
/// input (worker-interface.md §0 rule 4, §3). Returns the instance's message.
async fn deliver(instance: &TaskInstance, attempt: &AttemptRow, params: &SendMessageParams) -> anyhow::Result<String> {
    let core_task = attempt.core_task.clone();
    let (request, body) = (params.message_id.clone(), params.body.clone());
    let phase = {
        let id = core_task.clone();
        db(instance, move |db| db.read(|c| Ok(load_task(c, &id)?.phase))).await?
    };
    if phase == Phase::Accepting {
        let back = SendBack { request_id: request.clone(), task_id: core_task.clone(), reason: body };
        let (_, command) = db(instance, move |db| db.execute_recorded(&Caller::User, &back)).await?;
        return db(instance, move |db| {
            db.read(|c| {
                Ok(c.query_row(
                    "SELECT entity FROM event WHERE command_id = ?1 AND kind = 'message.queued'",
                    [&command],
                    |r| r.get(0),
                )?)
            })
        })
        .await;
    }
    let send =
        SendMessage { request_id: request.clone(), role: ROLE.into(), task_id: Some(core_task), body, reply_to: None };
    let id = db(instance, move |db| db.execute(&Caller::User, &send)).await?.id;
    // After a turn that did not end normally, the role holds until it is continued.
    let held = db(instance, |db| db.read(|c| hold(c, ROLE))).await?;
    match held {
        Some(Hold::Abnormal { .. }) => {
            let go = Continue { request_id: format!("{request}:continue"), role: ROLE.into() };
            ignore_rejection(db(instance, move |db| db.execute(&Caller::User, &go)).await)?;
        }
        Some(Hold::SessionUnidentified { .. }) => {
            let fresh = NewNativeSession { request_id: format!("{request}:session"), role: ROLE.into() };
            ignore_rejection(db(instance, move |db| db.execute(&Caller::User, &fresh)).await)?;
        }
        None => {}
    }
    Ok(id)
}

/// Asks the agent's running CLI to stop, once it has started. The turn then ends as interrupted.
fn interrupt_running(instance: &Arc<TaskInstance>) {
    let target = instance
        .running
        .lock()
        .unwrap()
        .iter()
        .find(|(_, t)| t.pid.is_some() && !t.interrupt_requested)
        .map(|(id, _)| id.clone());
    if let Some(turn_id) = target {
        let instance = instance.clone();
        tokio::spawn(async move {
            if let Err(e) = runner::interrupt(&instance, &turn_id).await {
                tracing::warn!(turn_id, error = format!("{e:#}"), "could not interrupt");
            }
        });
    }
}

// ---- the loop ----

async fn run(tasks: Arc<Tasks>, task_id: String, instance: Arc<TaskInstance>, stop: CancellationToken) {
    loop {
        match step(&tasks, &task_id, &instance).await {
            Ok(true) => {
                tracing::info!(task = task_id, "task finished");
                tasks.open.lock().unwrap().remove(&task_id);
                return;
            }
            Ok(false) => {}
            Err(e) => tracing::error!(task = task_id, error = format!("{e:#}"), "task step failed"),
        }
        tokio::select! {
            _ = instance.wake.notified() => {}
            _ = tokio::time::sleep(POLL) => {}
            _ = stop.cancelled() => return,
        }
    }
}

/// One round: reconcile, end what was asked to end, start what can start, then tell the
/// orchestrator what changed. Returns whether the task is finished: closed, with nothing left
/// running.
async fn step(tasks: &Tasks, task_id: &str, instance: &Arc<TaskInstance>) -> anyhow::Result<bool> {
    reconcile(instance).await?;
    // create_task opens the instance before it records the task.
    let id = task_id.to_owned();
    if tasks.read(move |c| store::task(c, &id)).await?.is_none() {
        return Ok(false);
    }
    settle_endings(tasks, task_id, instance).await?;
    let task = tasks.task_row(task_id).await.map_err(anyhow::Error::new)?;
    let attempt = tasks.attempt(&task.attempt_id).await.map_err(anyhow::Error::new)?;
    if attempt.ended.is_none() && attempt.ending.is_none() {
        let core_task = attempt.core_task.clone();
        let busy = db(instance, |db| db.read(|c| occupant(c, ROLE))).await?;
        // The role still works on an earlier attempt until that one has ended.
        if busy.as_deref().is_none_or(|t| t == core_task) {
            ignore_rejection(runtime(instance, StartAttempt { task_id: core_task }).await)?;
            if let Some(turn) = ignore_rejection(runtime(instance, RegisterTurn { role: ROLE.into() }).await)? {
                launch(instance, turn.turn_id);
            }
        }
    }
    launch_registered(instance).await?;
    observe(tasks, task_id, instance).await?;
    let task = tasks.task_row(task_id).await.map_err(anyhow::Error::new)?;
    let ending = {
        let id = task_id.to_owned();
        tasks.read(move |c| store::ending_attempts(c, &id)).await?
    };
    Ok(task.closed.is_some() && task.status.state.is_final() && ending.is_empty())
}

/// Ends the attempts asked to end once no turn of theirs is left unfinished: a stop takes effect
/// when the agent's process has exited (worker-interface.md §3, §9).
async fn settle_endings(tasks: &Tasks, task_id: &str, instance: &Arc<TaskInstance>) -> anyhow::Result<()> {
    let ending = {
        let id = task_id.to_owned();
        tasks.read(move |c| store::ending_attempts(c, &id)).await?
    };
    for attempt in ending {
        let unfinished = db(instance, |db| db.read(|c| unfinished_turn(c, ROLE))).await?;
        if let Some(turn) = unfinished.filter(|t| t.task_id.as_deref() == Some(attempt.core_task.as_str())) {
            match turn.state {
                // Never launched: it ends here.
                TurnState::Registered if !instance.running.lock().unwrap().contains_key(&turn.id) => {
                    runtime(instance, EndTurn { turn_id: turn.id, outcome: Outcome::Interrupted, failure: None })
                        .await?;
                }
                // Its CLI exits; an unknown one is reconciled once it has.
                _ => interrupt_running(instance),
            }
            continue;
        }
        let core_task = attempt.core_task.clone();
        let open = {
            let id = core_task.clone();
            !db(instance, move |db| db.read(|c| Ok(load_task(c, &id)?.phase.is_closed()))).await?
        };
        if open {
            let reason = match attempt.ending {
                Some(AttemptState::Abandoned) => "the orchestrator closed the task",
                _ => "the orchestrator stopped the attempt",
            };
            let end = Abandon { request_id: format!("{}:end", attempt.id), task_id: core_task, reason: reason.into() };
            db(instance, move |db| db.execute(&Caller::User, &end)).await?;
        }
        let status = Status::of(attempt.ending.unwrap_or(AttemptState::Canceled));
        let (id, attempt_id) = (task_id.to_owned(), attempt.id.clone());
        let events = tasks
            .write(move |tx| {
                store::set_ended(tx, &attempt_id, &status)?;
                Ok(vec![store::append(tx, &id, &attempt_id, status.event())?])
            })
            .await?;
        tasks.publish(events);
    }
    Ok(())
}

/// What the instance holds now, as the protocol says it.
struct Observed {
    status: Status,
    question: Option<Question>,
    receipts: Vec<(String, Receipt)>,
    mismatches: Vec<String>,
    core_seq: i64,
    /// The harness of the last turn, if it completed.
    completed_on: Option<String>,
}

/// Tells the orchestrator what changed in the task since it was last told: mismatches, the open
/// question, receipts, then the state (worker-interface.md §5).
async fn observe(tasks: &Tasks, task_id: &str, instance: &Arc<TaskInstance>) -> anyhow::Result<()> {
    let task = tasks.task_row(task_id).await.map_err(anyhow::Error::new)?;
    let attempt = tasks.attempt(&task.attempt_id).await.map_err(anyhow::Error::new)?;
    let messages = {
        let id = attempt.id.clone();
        tasks.read(move |c| store::attempt_messages(c, &id)).await?
    };
    let (core_seq, observed_attempt) = (task.core_seq, attempt.clone());
    let seen = db(instance, move |db| db.read(|c| read_instance(c, &observed_attempt, &messages, core_seq))).await?;
    if let Some(harness) = &seen.completed_on {
        tasks.host.quota_passed(harness);
    }
    let (id, attempt_id) = (task_id.to_owned(), attempt.id.clone());
    let events = tasks
        .write(move |tx| {
            let mut events = Vec::new();
            for body in seen.mismatches {
                events.push(store::append(tx, &id, &attempt_id, EventBody::MismatchFlagged { body })?);
            }
            store::set_core_seq(tx, &id, seen.core_seq)?;
            if task.question.as_ref().map(|q| &q.question_id) != seen.question.as_ref().map(|q| &q.question_id) {
                if let Some(old) = &task.question {
                    let body = EventBody::QuestionClosed { question_id: old.question_id.clone(), answered_by: None };
                    events.push(store::append(tx, &id, &attempt_id, body)?);
                }
                if let Some(question) = seen.question {
                    events.push(store::append(tx, &id, &attempt_id, EventBody::QuestionOpened { question })?);
                }
            }
            for (message_id, receipt) in seen.receipts {
                events.push(store::append(tx, &id, &attempt_id, EventBody::MessageReceipt { message_id, receipt })?);
            }
            if seen.status != task.status {
                events.push(store::append(tx, &id, &attempt_id, seen.status.event())?);
            }
            Ok(events)
        })
        .await?;
    tasks.publish(events);
    Ok(())
}

fn read_instance(
    c: &Connection,
    attempt: &AttemptRow,
    messages: &[MessageRow],
    core_seq: i64,
) -> lobotomy_core::Result<Observed> {
    let question = open_questions(c, Some(ROLE))?
        .into_iter()
        .find(|q| q.task_id.as_deref() == Some(attempt.core_task.as_str()))
        .map(|q| Question { question_id: q.id, body: q.body, options: Vec::new() });
    let mut receipts = Vec::new();
    for m in messages {
        // A delivered or undelivered message has its final receipt.
        if matches!(m.receipt, Receipt::Delivered | Receipt::NotDelivered { .. }) {
            continue;
        }
        let receipt = receipt(c, &m.core_id)?;
        if receipt != m.receipt {
            receipts.push((m.id.clone(), receipt));
        }
    }
    let mut mismatches = Vec::new();
    let mut last = core_seq;
    for event in lobotomy_core::view::events_after(c, core_seq)? {
        last = event.seq;
        if event.kind == "mismatch.flagged"
            && event.payload["task_id"].as_str() == Some(attempt.core_task.as_str())
            && let Some(body) = event.payload["body"].as_str()
        {
            mismatches.push(body.to_owned());
        }
    }
    let completed_on = last_turn(c, ROLE)?.filter(|t| t.outcome == Some(Outcome::Completed)).map(|t| t.harness);
    Ok(Observed { status: derive(c, attempt)?, question, receipts, mismatches, core_seq: last, completed_on })
}

/// What became of a message in the instance.
fn receipt(c: &Connection, core_id: &str) -> lobotomy_core::Result<Receipt> {
    let row: Option<(String, Option<String>, Option<i64>)> = c
        .query_row("SELECT state, turn_id, delivered_at FROM message WHERE id = ?1", [core_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    Ok(match row {
        Some((state, _, _)) if state == "queued" => Receipt::Queued,
        Some((state, _, _)) if state == "dropped" => {
            Receipt::NotDelivered { reason: "the attempt ended before the agent took it".into() }
        }
        Some((_, _, Some(_))) => Receipt::Delivered,
        Some((_, Some(turn), None)) => Receipt::Bound { turn },
        _ => Receipt::NotDelivered { reason: "the message is gone".into() },
    })
}

/// The attempt's state, from its task in the instance (worker-interface.md §4).
fn derive(c: &Connection, attempt: &AttemptRow) -> lobotomy_core::Result<Status> {
    if let Some(ended) = &attempt.ended {
        return Ok(ended.clone());
    }
    let task = load_task(c, &attempt.core_task)?;
    let blocked = |need, question_id, reason: String| Status {
        state: AttemptState::Blocked,
        blocked: Some(Blocked { need, question_id }),
        reason: Some(reason),
    };
    Ok(match task.phase {
        Phase::Queued => Status::of(AttemptState::Submitted),
        Phase::Accepting => Status::of(AttemptState::Delivered),
        Phase::Done => Status::of(AttemptState::Adopted),
        Phase::Abandoned => Status::of(attempt.ending.unwrap_or(AttemptState::Abandoned)),
        Phase::Executing => {
            let unfinished = unfinished_turn(c, ROLE)?.filter(|t| t.task_id.as_deref() == Some(task.id.as_str()));
            if unfinished.as_ref().is_some_and(|t| t.state == TurnState::Unknown) {
                Status::of(AttemptState::Unknown)
            } else if let Some(q) =
                open_questions(c, Some(ROLE))?.into_iter().find(|q| q.task_id.as_deref() == Some(task.id.as_str()))
            {
                blocked(Need::Input, Some(q.id), q.body)
            } else {
                match hold(c, ROLE)? {
                    Some(Hold::Abnormal { turn }) => {
                        let reason = match (&turn.failure, turn.outcome) {
                            (Some(f), _) => f.message.clone(),
                            (None, Some(Outcome::Interrupted)) => "the turn was interrupted".to_owned(),
                            _ => "the turn did not end normally".to_owned(),
                        };
                        blocked(Need::Resume, None, reason)
                    }
                    Some(Hold::SessionUnidentified { .. }) => blocked(
                        Need::Resume,
                        None,
                        "the harness's session id was not recorded; the next message starts a new session".to_owned(),
                    ),
                    // Nothing runs and nothing waits to: the agent ended its turn without reporting
                    // done or asking.
                    None if unfinished.is_none() && queued_messages(c, ROLE)?.is_empty() => blocked(
                        Need::Resume,
                        None,
                        "the agent ended its turn without reporting done or asking".to_owned(),
                    ),
                    None => Status::of(AttemptState::Working),
                }
            }
        }
    })
}
