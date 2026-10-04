//! The scheduler: starts attempts, registers turns and reconciles unknown turns
//! (data-model.md §3.3, §9.2 runtime commands).
//!
//! Each step is a runtime command whose preconditions live in the command itself; a rejection
//! just means there is nothing to do for that role now.

use std::sync::Arc;
use std::time::Duration;

use lobotomy_core::id::now_ms;
use lobotomy_core::role::list_roles;
use lobotomy_core::task::{StartAttempt, next_queued_task, occupant};
use lobotomy_core::turn::{
    Continue, EndTurn, FailureKind, MarkUnfinishedUnknown, Outcome, RegisterTurn, Turn, TurnState, last_turn,
    turns_in_state,
};
use lobotomy_harness::process;
use tokio_util::sync::CancellationToken;

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
    check_quota(project)?;
    let roles = db(project, |db| db.read(list_roles)).await?;
    for role in roles {
        // A blocked quota domain starts nothing new; running turns end on their own
        // (data-model.md §8.3).
        if project.host.is_blocked(&role.harness)? {
            continue;
        }
        let name = role.name.clone();
        let (busy, next, last) = db(project, move |db| {
            db.read(|c| Ok((occupant(c, &name)?.is_some(), next_queued_task(c, &name)?, last_turn(c, &name)?)))
        })
        .await?;
        if !busy && let Some(task) = next {
            ignore_rejection(runtime(project, StartAttempt { task_id: task.id }).await)?;
        }
        // After the quota recovers, a role whose last turn failed for quota continues on its
        // own. A user pause still holds: continue is then rejected (data-model.md §8.5).
        if let Some(last) = last.filter(failed_for_quota) {
            let resume = Continue { request_id: format!("quota-continue-{}", last.id), role: role.name.clone() };
            if let Some(turn) = ignore_rejection(runtime(project, resume).await)? {
                launch(project, turn.turn_id);
            }
            continue;
        }
        if let Some(turn) = ignore_rejection(runtime(project, RegisterTurn { role: role.name.clone() }).await)? {
            launch(project, turn.turn_id);
        }
    }
    // Turns registered by a user command, such as continue, still need a runner.
    for turn in db(project, |db| db.read(|c| turns_in_state(c, TurnState::Registered))).await? {
        if !project.host.is_blocked(&turn.harness)? {
            launch(project, turn.id);
        }
    }
    Ok(())
}

fn failed_for_quota(turn: &Turn) -> bool {
    turn.outcome == Some(Outcome::Failed) && turn.failure.as_ref().is_some_and(|f| f.kind == FailureKind::Quota)
}

/// Starts the planned quota checks that are due (data-model.md §8.4). A check runs on its own
/// task and wakes the scheduler when it is done.
fn check_quota(project: &Arc<Project>) -> anyhow::Result<()> {
    for domain in project.host.db.due(now_ms())? {
        let project = project.clone();
        tokio::spawn(async move {
            if let Err(e) = project.host.check(&domain.harness).await {
                tracing::error!(harness = domain.harness, error = format!("{e:#}"), "quota check failed");
            }
            project.wake.notify_one();
        });
    }
    Ok(())
}

/// Ends unknown turns whose CLI has exited (data-model.md §3.3). A CLI that still runs keeps its
/// turn unknown; the runtime never kills it on its own.
async fn reconcile(project: &Arc<Project>) -> anyhow::Result<()> {
    for turn in db(project, |db| db.read(|c| turns_in_state(c, TurnState::Unknown))).await? {
        let alive = match (turn.pid, turn.process_start) {
            (Some(pid), Some(start)) => process::is_running(pid as u32, start),
            // No pid: the CLI never ran. It is recorded before the process is resumed.
            _ => false,
        };
        if !alive {
            runtime(project, EndTurn { turn_id: turn.id, outcome: Outcome::Interrupted, failure: None }).await?;
        }
    }
    Ok(())
}

/// A rejected runtime command means its preconditions do not hold now.
fn ignore_rejection<T>(result: anyhow::Result<T>) -> anyhow::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.downcast_ref::<lobotomy_core::Error>().is_some_and(|e| e.code().is_some()) => Ok(None),
        Err(e) => Err(e),
    }
}
