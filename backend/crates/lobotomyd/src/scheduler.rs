//! The scheduler: reconciles unknown turns, starts attempts and registers turns (data-model.md
//! §3.3, §9.2 runtime commands).
//!
//! Each step is a runtime command whose preconditions live in the command itself; a rejection
//! just means there is nothing to do for that role now.

use std::sync::Arc;
use std::time::Duration;

use lobotomy_core::project::load_project;
use lobotomy_core::role::list_roles;
use lobotomy_core::task::{StartAttempt, next_queued_task, occupant};
use lobotomy_core::turn::{EndTurn, MarkUnfinishedUnknown, Outcome, RegisterTurn, TurnState, turns_in_state};
use lobotomy_harness::process;
use tokio_util::sync::CancellationToken;

use crate::instance::Instance;
use crate::project::Project;
use crate::runner::{db, launch, runtime};

/// How often the scheduler looks again without being woken, mainly to see whether the CLI of an
/// unknown turn has exited.
const POLL: Duration = Duration::from_secs(5);

/// Turns that were registered or running when the backend stopped become unknown
/// (data-model.md §3.2). Call once before [`run`].
pub async fn recover(project: &Arc<Project>) -> anyhow::Result<()> {
    let unknown = runtime(project, MarkUnfinishedUnknown {}).await?;
    if !unknown.is_empty() {
        tracing::info!(turns = ?unknown, "turns left unfinished by the last run are unknown until reconciled");
    }
    Ok(())
}

pub async fn run(project: Arc<Project>, shutdown: CancellationToken) {
    loop {
        if let Err(e) = tick(&project).await {
            tracing::error!(error = format!("{e:#}"), "scheduler step failed");
        }
        tokio::select! {
            _ = project.wake.notified() => {}
            _ = tokio::time::sleep(POLL) => {}
            _ = shutdown.cancelled() => return,
        }
    }
}

async fn tick(project: &Arc<Project>) -> anyhow::Result<()> {
    reconcile(project).await?;
    // Nothing runs before a repository is connected.
    if db(project, |db| db.read(load_project)).await?.is_none() {
        return Ok(());
    }
    let roles = db(project, |db| db.read(list_roles)).await?;
    for role in roles {
        // A blocked quota domain starts nothing new; running turns end on their own. Only the
        // user's retry opens the domain again (data-model.md §8.3, §8.4).
        if project.host.is_blocked(&role.harness)? {
            continue;
        }
        let name = role.name.clone();
        let (busy, next) =
            db(project, move |db| db.read(|c| Ok((occupant(c, &name)?.is_some(), next_queued_task(c, &name)?))))
                .await?;
        if !busy && let Some(task) = next {
            ignore_rejection(runtime(project, StartAttempt { task_id: task.id }).await)?;
        }
        if let Some(turn) = ignore_rejection(runtime(project, RegisterTurn { role: role.name.clone() }).await)? {
            launch(&project.instance, turn.turn_id);
        }
    }
    launch_registered(&project.instance).await
}

/// Turns registered by a user command, such as continue, still need a runner.
pub(crate) async fn launch_registered(instance: &Arc<Instance>) -> anyhow::Result<()> {
    for turn in db(instance, |db| db.read(|c| turns_in_state(c, TurnState::Registered))).await? {
        if !instance.host.is_blocked(&turn.harness)? {
            launch(instance, turn.id);
        }
    }
    Ok(())
}

/// Ends unknown turns whose CLI has exited (data-model.md §3.3). A CLI that still runs keeps its
/// turn unknown; the runtime never kills it on its own.
pub(crate) async fn reconcile(instance: &Instance) -> anyhow::Result<()> {
    for turn in db(instance, |db| db.read(|c| turns_in_state(c, TurnState::Unknown))).await? {
        let alive = match (turn.pid, turn.process_start) {
            (Some(pid), Some(start)) => process::is_running(pid as u32, start),
            // No pid: the CLI never ran. It is recorded before the process is resumed.
            _ => false,
        };
        if !alive {
            runtime(instance, EndTurn { turn_id: turn.id, outcome: Outcome::Interrupted, failure: None }).await?;
        }
    }
    Ok(())
}

/// A rejected runtime command means its preconditions do not hold now. Anything else, a broken
/// invariant included, is an error: waiting does not resolve it (#16).
pub(crate) fn ignore_rejection<T>(result: anyhow::Result<T>) -> anyhow::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) => match e.downcast_ref::<lobotomy_core::Error>().and_then(|e| e.code()) {
            Some(code) => {
                tracing::debug!(code, "not now: {e:#}");
                Ok(None)
            }
            None => Err(e),
        },
    }
}
