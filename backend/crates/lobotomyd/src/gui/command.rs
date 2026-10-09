//! The commands the GUI sends: the user's commands (data-model.md §9.2), which go through the
//! command layer with the GUI's request id as idempotency key, and the runtime actions the user
//! may trigger.

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
use serde_json::{Value, json};

use crate::project::Project;
use crate::results;
use crate::runner::db;

#[derive(Deserialize)]
pub struct CommandParams {
    name: String,
    #[serde(default)]
    args: Value,
}

async fn user<C>(project: &Arc<Project>, args: Value) -> anyhow::Result<Value>
where
    C: Command + serde::de::DeserializeOwned + Send + 'static,
    C::Output: Send + 'static,
{
    let cmd: C = serde_json::from_value(args).with_context(|| format!("arguments of {}", C::NAME))?;
    let out = db(project, move |db: &Db| db.execute(&Caller::User, &cmd)).await?;
    Ok(serde_json::to_value(out)?)
}

#[derive(Deserialize)]
struct TurnParams {
    turn_id: String,
}

#[derive(Deserialize)]
struct OnboardParams {
    repo_path: String,
}

#[derive(Deserialize)]
struct HarnessParams {
    harness: String,
}

#[derive(Deserialize)]
struct PermissionParams {
    harness: String,
    permission: String,
}

pub async fn run(project: &Arc<Project>, CommandParams { name, args }: CommandParams) -> anyhow::Result<Value> {
    macro_rules! user_commands {
        ($($ty:ty),* $(,)?) => {
            $(if name == <$ty as Command>::NAME { return user::<$ty>(project, args).await; })*
        };
    }
    user_commands!(
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
    match name.as_str() {
        "onboard" => {
            let OnboardParams { repo_path } = serde_json::from_value(args)?;
            crate::onboard::onboard(project, std::path::Path::new(&repo_path)).await?;
            Ok(Value::Null)
        }
        "interrupt" => {
            let TurnParams { turn_id } = serde_json::from_value(args)?;
            crate::runner::interrupt(project, &turn_id).await?;
            Ok(Value::Null)
        }
        // "终止残留进程" (data-model.md §3.3): only the CLI of an unknown turn that still runs.
        "terminate" => {
            let TurnParams { turn_id } = serde_json::from_value(args)?;
            let id = turn_id.clone();
            let turn = db(project, move |db| db.read(|c| load_turn(c, &id))).await?;
            let (TurnState::Unknown, Some(pid), Some(start)) = (turn.state, turn.pid, turn.process_start) else {
                return Err(Error::rejected(
                    "not_unknown_turn",
                    format!("turn {turn_id} is not an unknown turn with a known process"),
                )
                .into());
            };
            Ok(json!({ "terminated": lobotomy_harness::process::terminate(pid as u32, start)? }))
        }
        "quota_retry" => {
            let HarnessParams { harness } = serde_json::from_value(args)?;
            Ok(serde_json::to_value(project.host.retry(&harness).await?)?)
        }
        // A host setting, not a project command: it has no event, so the windows are told to
        // take a new snapshot (harness-adapter.md §1.9).
        "set_permission" => {
            let PermissionParams { harness, permission } = serde_json::from_value(args)?;
            let set = project.host.set_permission(&harness, &permission)?;
            let _ = project.gui_push.send(json!({ "type": "host" }).to_string().into());
            Ok(json!({ "harness": harness, "permission": set.as_str() }))
        }
        "retry_failed" => Ok(json!({ "retried": results::retry_failed(project) })),
        "shutdown" => {
            project.shutdown_requested.notify_one();
            Ok(Value::Null)
        }
        other => Err(Error::rejected("bad_request", format!("unknown command {other}")).into()),
    }
}
