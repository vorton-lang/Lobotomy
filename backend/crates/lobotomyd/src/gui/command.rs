//! The commands the GUI sends: the user's commands (data-model.md §9.2), which go through the
//! command layer with the GUI's request id as idempotency key, the runtime actions the user
//! may trigger, and the host's commands, which concern no single project (frontend.md §3 rule 8).

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use lobotomy_core::capture::{ApproveNewFiles, DiscardOutsideChanges, DiscardUncaptured};
use lobotomy_core::project::EditProjectConfig;
use lobotomy_core::task::{
    Abandon, AdoptOutsideChanges, CreateTask, EditCriteria, MoveInQueue, Reopen, SendMessage, SetPaused,
};
use lobotomy_core::turn::{Continue, NewNativeSession, SetRoleHarness, TurnState, load_turn};
use lobotomy_core::verify::{Accept, RetryPreview, SendBack};
use lobotomy_core::{Caller, Command, Db, Error};
use serde::Deserialize;
use serde_json::Value;

use crate::host::Host;
use crate::instance::Instance;
use crate::project::Project;
use crate::projects::Projects;
use crate::results;
use crate::runner::db;

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
    permission: String,
}

/// Runs a host command; `None` when `name` is a project command.
pub async fn host(host: &Arc<Host>, projects: &Projects, params: &CommandParams) -> Option<anyhow::Result<Value>> {
    let args = params.args.clone();
    let result = match params.name.as_str() {
        // A new project, connected while the backend runs (data-model.md §10.4).
        "create_project" => {
            async {
                let CreateProjectParams { repo_path } = serde_json::from_value(args)?;
                Ok(serde_json::to_value(projects.create(Path::new(&repo_path)).await?)?)
            }
            .await
        }
        // Archiving stops the project and keeps its data; unarchiving and opening again open it as
        // at startup (data-model.md §10.3, §10.4).
        "archive_project" | "unarchive_project" | "retry_open" => {
            async {
                let ProjectParams { project_id } = serde_json::from_value(args)?;
                match params.name.as_str() {
                    "archive_project" => projects.archive(&project_id).await?,
                    "unarchive_project" => projects.unarchive(&project_id).await?,
                    _ => projects.retry_open(&project_id).await?,
                }
                Ok(Value::Null)
            }
            .await
        }
        // A domain that opens lets the roles of every project go on.
        "quota_retry" => {
            async {
                let HarnessParams { harness } = serde_json::from_value(args)?;
                let domain = host.retry(&harness).await?;
                projects.wake_all();
                Ok(serde_json::to_value(domain)?)
            }
            .await
        }
        // A host setting, not a project command: it has no event, so the windows are told to
        // take a new snapshot (harness-adapter.md §1.9).
        "set_permission" => (|| {
            let PermissionParams { harness, permission } = serde_json::from_value(args)?;
            let set = host.set_permission(&harness, &permission)?;
            host.changed();
            Ok(serde_json::to_value(PermissionChanged { harness, permission: set.as_str() })?)
        })(),
        "shutdown" => {
            host.shutdown_requested.notify_one();
            Ok(Value::Null)
        }
        _ => return None,
    };
    Some(result)
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
            ApproveNewFiles,
            DiscardUncaptured,
            AdoptOutsideChanges,
            DiscardOutsideChanges,
            RetryPreview,
            EditProjectConfig,
        );
    };
}

/// Runs a command of one project.
pub async fn run(project: &Arc<Project>, CommandParams { name, args, .. }: CommandParams) -> anyhow::Result<Value> {
    macro_rules! user_commands {
        ($($ty:ty),* $(,)?) => {
            $(if name == <$ty as Command>::NAME { return user::<$ty>(project, args).await; })*
        };
    }
    project_commands!(user_commands);
    match name.as_str() {
        "start_trial" => Ok(serde_json::to_value(crate::trial::start(project, serde_json::from_value(args)?).await?)?),
        "stop_trial" => {
            let StopTrial { trial_id } = serde_json::from_value(args)?;
            project.trials.stop(&trial_id).await?;
            Ok(Value::Null)
        }
        "retry_failed" => Ok(serde_json::to_value(RetryFailed { retried: results::retry_failed(project) })?),
        _ => turn_actions(project, &name, args).await,
    }
}

/// Runs a command about Angela (data-model.md §10.6): her messages and turns. She has no tasks,
/// slot or results.
pub async fn run_angela(angela: &Instance, CommandParams { name, args, .. }: CommandParams) -> anyhow::Result<Value> {
    macro_rules! user_commands {
        ($($ty:ty),* $(,)?) => {
            $(if name == <$ty as Command>::NAME { return user::<$ty>(angela, args).await; })*
        };
    }
    user_commands!(SendMessage, Continue, NewNativeSession, SetRoleHarness);
    turn_actions(angela, &name, args).await
}

/// The runtime actions on a turn that the user may trigger, in any instance.
async fn turn_actions(instance: &Instance, name: &str, args: Value) -> anyhow::Result<Value> {
    match name {
        "interrupt" => {
            let TurnParams { turn_id } = serde_json::from_value(args)?;
            crate::runner::interrupt(instance, &turn_id).await?;
            Ok(Value::Null)
        }
        // "终止残留进程" (data-model.md §3.3): only the CLI of an unknown turn that still runs.
        "terminate" => {
            let TurnParams { turn_id } = serde_json::from_value(args)?;
            let id = turn_id.clone();
            let turn = db(instance, move |db| db.read(|c| load_turn(c, &id))).await?;
            let (TurnState::Unknown, Some(pid), Some(start)) = (turn.state, turn.pid, turn.process_start) else {
                return Err(Error::rejected(
                    "not_unknown_turn",
                    format!("turn {turn_id} is not an unknown turn with a known process"),
                )
                .into());
            };
            Ok(serde_json::to_value(Terminated {
                terminated: lobotomy_harness::process::terminate(pid as u32, start)?,
            })?)
        }
        other => Err(Error::rejected("bad_request", format!("unknown command {other}")).into()),
    }
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct StopTrial {
    trial_id: String,
}

#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct PermissionChanged {
    harness: String,
    permission: &'static str,
}

#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct RetryFailed {
    retried: usize,
}

#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct Terminated {
    terminated: bool,
}

#[cfg(feature = "ts-rs")]
pub(super) fn bindings(out: &mut super::bindings::Bindings) {
    use super::protocol::NoParams;
    macro_rules! export {
        ($($ty:ty),* $(,)?) => { $(out.command::<$ty>();)* };
    }
    project_commands!(export);
    out.action::<CreateProjectParams, lobotomy_core::host::ProjectEntry>("create_project");
    for name in ["archive_project", "unarchive_project", "retry_open"] {
        out.action::<ProjectParams, ()>(name);
    }
    out.action::<HarnessParams, lobotomy_core::quota::Domain>("quota_retry");
    out.action::<PermissionParams, PermissionChanged>("set_permission");
    out.action::<NoParams, ()>("shutdown");
    out.action::<crate::trial::StartTrial, crate::trial::TrialView>("start_trial");
    out.action::<StopTrial, ()>("stop_trial");
    out.action::<NoParams, RetryFailed>("retry_failed");
    out.action::<TurnParams, ()>("interrupt");
    out.action::<TurnParams, Terminated>("terminate");
}
