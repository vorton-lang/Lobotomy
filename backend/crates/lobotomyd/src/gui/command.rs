//! The commands the GUI sends: the user's commands (data-model.md §9.2), which go through the
//! command layer with the GUI's request id as idempotency key, the runtime actions the user
//! may trigger, and the host's commands, which concern no single project (frontend.md §3 rule 8).

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use lobotomy_agent::runner::db;
use lobotomy_core::role::SetRoleModel;
use lobotomy_core::task::{
    Abandon, Accept, CreateTask, EditCriteria, MoveInQueue, Reopen, SendBack, SendMessage, SetPaused,
};
use lobotomy_core::turn::{Continue, NewNativeSession, SetRoleHarness, TurnState, load_turn};
use lobotomy_core::{Caller, Command, Db, Error};
use serde::Deserialize;
use serde_json::Value;

use crate::host::Host;
use crate::instance::Instance;
use crate::project::Project;
use crate::projects::Projects;
use crate::workers::{Authority, worker_of};

#[derive(Deserialize)]
pub struct CommandParams {
    name: String,
    #[serde(default)]
    args: Value,
    /// The project a project command is for; the host's commands ignore it.
    #[serde(default)]
    pub project: Option<String>,
}

async fn user<C>(instance: &Instance, args: Value) -> anyhow::Result<Value>
where
    C: Command + serde::de::DeserializeOwned + Send + 'static,
    C::Output: Send + 'static,
{
    let cmd: C = serde_json::from_value(args).with_context(|| format!("arguments of {}", C::NAME))?;
    let out = db(instance, move |db: &Db| db.execute(&Caller::User, &cmd)).await?;
    Ok(serde_json::to_value(out)?)
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct TurnParams {
    turn_id: String,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct CreateProjectParams {
    repo_path: String,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct ProjectParams {
    project_id: String,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct HarnessParams {
    harness: String,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct PermissionParams {
    harness: String,
    #[cfg_attr(feature = "ts-rs", ts(type = "\"full\" | \"auto_review\""))]
    permission: String,
}

/// Runs a host command; `None` when `name` is a project command.
pub async fn host(host: &Arc<Host>, projects: &Projects, params: &CommandParams) -> anyhow::Result<Option<Value>> {
    let args = params.args.clone();
    Ok(Some(match params.name.as_str() {
        // A new project, connected while the backend runs (data-model.md §10.4).
        name if name == CREATE_PROJECT.name => {
            let CreateProjectParams { repo_path } = CREATE_PROJECT.decode(args)?;
            CREATE_PROJECT.encode(projects.create(Path::new(&repo_path)).await?)?
        }
        // Archiving stops the project and keeps its data; unarchiving and opening again open it as
        // at startup (data-model.md §10.3, §10.4).
        name if name == ARCHIVE_PROJECT.name => {
            let ProjectParams { project_id } = ARCHIVE_PROJECT.decode(args)?;
            projects.archive(&project_id).await?;
            ARCHIVE_PROJECT.encode(())?
        }
        name if name == UNARCHIVE_PROJECT.name => {
            let ProjectParams { project_id } = UNARCHIVE_PROJECT.decode(args)?;
            projects.unarchive(&project_id).await?;
            UNARCHIVE_PROJECT.encode(())?
        }
        name if name == RETRY_OPEN.name => {
            let ProjectParams { project_id } = RETRY_OPEN.decode(args)?;
            projects.retry_open(&project_id).await?;
            RETRY_OPEN.encode(())?
        }
        // A domain that opens lets the roles of every project go on.
        name if name == QUOTA_RETRY.name => {
            let HarnessParams { harness } = QUOTA_RETRY.decode(args)?;
            let domain = host.retry(&harness).await?;
            projects.wake_all();
            QUOTA_RETRY.encode(domain)?
        }
        // A host setting, not a project command: it has no event, so the windows are told to
        // take a new snapshot (harness-adapter.md §1.9).
        name if name == SET_PERMISSION.name => {
            let PermissionParams { harness, permission } = SET_PERMISSION.decode(args)?;
            let set = host.set_permission(&harness, &permission)?;
            host.changed();
            SET_PERMISSION.encode(PermissionChanged { harness, permission: set.as_str() })?
        }
        name if name == SHUTDOWN.name => {
            host.shutdown_requested.notify_one();
            SHUTDOWN.encode(())?
        }
        _ => return Ok(None),
    }))
}

macro_rules! project_commands {
    ($callback:ident) => {
        $callback!(
            CreateTask,
            EditCriteria,
            SendMessage,
            SetPaused,
            MoveInQueue,
            Abandon,
            Reopen,
            Accept,
            SendBack,
            Continue,
            NewNativeSession,
            SetRoleHarness,
            SetRoleModel,
        );
    };
}

/// Runs a command of one project.
pub async fn run(project: &Arc<Project>, CommandParams { name, args, .. }: CommandParams) -> anyhow::Result<Value> {
    if let Some(done) = worker_command(project, &name, &args).await? {
        return Ok(done);
    }
    macro_rules! user_commands {
        ($($ty:ty),* $(,)?) => {
            $(if name == <$ty as Command>::NAME { return user::<$ty>(project, args).await; })*
        };
    }
    project_commands!(user_commands);
    turn_actions(project, &name, args).await
}

/// The user's commands on a task a worker runs go to the worker (tech-plan.md §7 step 1): accepting
/// merges its change, a send-back or a message goes to it, pausing interrupts its turn, abandoning
/// stops it, and new criteria start a new attempt. `None` for any other command or task.
async fn worker_command(project: &Arc<Project>, name: &str, args: &Value) -> anyhow::Result<Option<Value>> {
    let Some(workers) = project.host.workers.get() else { return Ok(None) };
    let Some(task_id) = args.get("task_id").and_then(Value::as_str) else { return Ok(None) };
    let Some(worker) = worker_of(project, task_id).await? else { return Ok(None) };
    let worker = &worker;
    // A message to the worker is recorded in the project, then sent.
    let send = |message: SendMessage| async move {
        if message.role != worker.worker {
            return Err(Error::rejected("wrong_executor", "消息的 role 必须是任务的 worker").into());
        }
        let created = db(project, move |db| db.execute(&Caller::User, &message)).await?;
        workers.forward(project, worker, task_id, &created.id).await?;
        anyhow::Ok(created)
    };
    Ok(Some(match name {
        n if n == Accept::NAME => {
            let accept: Accept = serde_json::from_value(args.clone()).with_context(|| format!("arguments of {n}"))?;
            let caller = Caller::User;
            let (version, dropping) = (accept.criteria_version, accept.dropping);
            workers.adopt(project, caller, worker, task_id, version, dropping, Authority::User).await?;
            Value::Null
        }
        n if n == SendBack::NAME => {
            let back: SendBack = serde_json::from_value(args.clone()).with_context(|| format!("arguments of {n}"))?;
            send(SendMessage {
                request_id: back.request_id,
                role: worker.worker.clone(),
                task_id: Some(task_id.to_owned()),
                body: back.reason,
                reply_to: None,
                stop_id: None,
            })
            .await?;
            Value::Null
        }
        n if n == SendMessage::NAME => {
            let message: SendMessage =
                serde_json::from_value(args.clone()).with_context(|| format!("arguments of {n}"))?;
            serde_json::to_value(send(message).await?)?
        }
        n if n == Abandon::NAME => {
            let abandon: Abandon = serde_json::from_value(args.clone()).with_context(|| format!("arguments of {n}"))?;
            workers.abandon(project, Caller::User, worker, task_id, abandon.reason).await?;
            Value::Null
        }
        n if n == SetPaused::NAME => {
            let pause: SetPaused = serde_json::from_value(args.clone()).with_context(|| format!("arguments of {n}"))?;
            if !pause.paused {
                return Err(Error::rejected("worker_resumes_on_message", "发一条消息，worker 就接着做").into());
            }
            workers.interrupt(worker, task_id).await?;
            Value::Null
        }
        n if n == EditCriteria::NAME => {
            let edit: EditCriteria =
                serde_json::from_value(args.clone()).with_context(|| format!("arguments of {n}"))?;
            let edited = db(project, move |db| db.execute(&Caller::User, &edit)).await?;
            workers.revise(project, worker, task_id).await?;
            serde_json::to_value(edited)?
        }
        _ => return Ok(None),
    }))
}

/// Runs a command about Angela (data-model.md §10.6): her messages and turns. She has no tasks.
pub async fn run_angela(angela: &Instance, CommandParams { name, args, .. }: CommandParams) -> anyhow::Result<Value> {
    macro_rules! user_commands {
        ($($ty:ty),* $(,)?) => {
            $(if name == <$ty as Command>::NAME { return user::<$ty>(angela, args).await; })*
        };
    }
    user_commands!(SendMessage, Continue, NewNativeSession, SetRoleHarness, SetRoleModel);
    turn_actions(angela, &name, args).await
}

/// The runtime actions on a turn that the user may trigger, in any instance.
async fn turn_actions(instance: &Instance, name: &str, args: Value) -> anyhow::Result<Value> {
    match name {
        name if name == INTERRUPT.name => {
            let TurnParams { turn_id } = INTERRUPT.decode(args)?;
            lobotomy_agent::runner::interrupt(instance, &turn_id).await?;
            INTERRUPT.encode(())
        }
        // "终止残留进程" (data-model.md §3.3): only the CLI of an unknown turn that still runs.
        name if name == TERMINATE.name => {
            let TurnParams { turn_id } = TERMINATE.decode(args)?;
            let id = turn_id.clone();
            let turn = db(instance, move |db| db.read(|c| load_turn(c, &id))).await?;
            let (TurnState::Unknown, Some(pid), Some(start)) = (turn.state, turn.pid, turn.process_start) else {
                return Err(Error::rejected(
                    "not_unknown_turn",
                    format!("turn {turn_id} is not an unknown turn with a known process"),
                )
                .into());
            };
            TERMINATE.encode(Terminated { terminated: lobotomy_harness::process::terminate(pid as u32, start)? })
        }
        other => Err(Error::rejected("bad_request", format!("unknown command {other}")).into()),
    }
}

#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct PermissionChanged {
    harness: String,
    #[cfg_attr(feature = "ts-rs", ts(type = "\"full\" | \"auto_review\""))]
    permission: &'static str,
}

#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct Terminated {
    terminated: bool,
}

#[cfg(feature = "ts-rs")]
pub(super) fn bindings(out: &mut super::bindings::Bindings) {
    macro_rules! export {
        ($($ty:ty),* $(,)?) => { $(out.command::<$ty>();)* };
    }
    project_commands!(export);
    runtime_bindings(out);
}

super::protocol::routes! {
    runtime_bindings, action;
    CREATE_PROJECT: CreateProjectParams => lobotomy_core::host::ProjectEntry = "create_project";
    ARCHIVE_PROJECT: ProjectParams => () = "archive_project";
    UNARCHIVE_PROJECT: ProjectParams => () = "unarchive_project";
    RETRY_OPEN: ProjectParams => () = "retry_open";
    QUOTA_RETRY: HarnessParams => lobotomy_core::quota::Domain = "quota_retry";
    SET_PERMISSION: PermissionParams => PermissionChanged = "set_permission";
    SHUTDOWN: super::protocol::NoParams => () = "shutdown";
    INTERRUPT: TurnParams => () = "interrupt";
    TERMINATE: TurnParams => Terminated = "terminate";
}
