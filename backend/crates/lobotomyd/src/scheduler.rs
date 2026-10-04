//! The scheduler: reconciles unknown turns, hands store work to jobs, starts attempts and
//! registers turns (data-model.md §3.3, §9.2 runtime commands).
//!
//! Each step is a runtime command whose preconditions live in the command itself; a rejection
//! just means there is nothing to do for that role now.

use std::sync::Arc;
use std::time::Duration;

use lobotomy_core::capture::pending_captures;
use lobotomy_core::project::load_project;
use lobotomy_core::role::list_roles;
use lobotomy_core::task::{Phase, StartAttempt, next_queued_task, occupant};
use lobotomy_core::turn::{EndTurn, MarkUnfinishedUnknown, Outcome, RegisterTurn, TurnState, turns_in_state};
use lobotomy_core::verify::{
    Reverify, StartVerification, evidence_is_stale, latest_verification, next_preview, running_verifications, tasks_in_phase,
};
use lobotomy_core::workspace::{AlignIdleSlot, materializing};
use lobotomy_harness::process;
use tokio_util::sync::CancellationToken;

use crate::project::Project;
use crate::results::{self, spawn_job};
use crate::runner::{db, launch, runtime};

/// How often the scheduler looks again without being woken, mainly to see whether the CLI of an
/// unknown turn has exited.
const POLL: Duration = Duration::from_secs(5);

/// Turns that were registered or running when the backend stopped become unknown
/// (data-model.md §3.2), and what nothing owns any more is deleted (data-model.md §6). Call once
/// before [`run`].
pub async fn recover(project: &Arc<Project>) -> anyhow::Result<()> {
    let unknown = runtime(project, MarkUnfinishedUnknown {}).await?;
    if !unknown.is_empty() {
        tracing::info!(turns = ?unknown, "turns left unfinished by the last run are unknown until reconciled");
    }
    results::sweep(project).await
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
    // Nothing runs before a repository is connected: there is no integration version to work on.
    if db(project, |db| db.read(load_project)).await?.is_none() {
        return Ok(());
    }
    store_work(project).await?;
    let roles = db(project, |db| db.read(list_roles)).await?;
    for role in roles {
        // A blocked quota domain starts nothing new; running turns end on their own. Only the
        // user's retry opens the domain again (data-model.md §8.3, §8.4).
        if project.host.is_blocked(&role.harness)? {
            continue;
        }
        let name = role.name.clone();
        let (busy, next) = db(project, move |db| {
            db.read(|c| Ok((occupant(c, &name)?.is_some(), next_queued_task(c, &name)?)))
        })
        .await?;
        if !busy && let Some(task) = next {
            let code_start = results::code_start(project, &task.id).await?;
            ignore_rejection(runtime(project, StartAttempt { task_id: task.id, code_start }).await)?;
        }
        // Without a task the slot follows the integration version (harness-adapter.md §3).
        if role.slot.is_some() {
            ignore_rejection(runtime(project, AlignIdleSlot { role: role.name.clone() }).await)?;
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

/// Hands each pending piece of store work to a job: slots to write, turns to capture, candidates
/// to verify, the preview to write.
async fn store_work(project: &Arc<Project>) -> anyhow::Result<()> {
    for ws in db(project, |db| db.read(materializing)).await? {
        spawn_job(project, format!("slot:{}", ws.name), results::materialize(project.clone(), ws));
    }
    for capture in db(project, |db| db.read(pending_captures)).await? {
        spawn_job(project, format!("capture:{}", capture.id), results::capture(project.clone(), capture));
    }
    // A running verification left by the last run is run again; checks can be repeated
    // (data-model.md §5).
    for v in db(project, |db| db.read(running_verifications)).await? {
        spawn_job(project, format!("verify:{}", v.id), results::verify(project.clone(), v.id));
    }
    for task in db(project, |db| db.read(|c| tasks_in_phase(c, Phase::Verifying))).await? {
        if let Some(id) = ignore_rejection(runtime(project, StartVerification { task_id: task }).await)? {
            spawn_job(project, format!("verify:{id}"), results::verify(project.clone(), id));
        }
    }
    // Evidence is bound to the integration version and the checks it ran with (data-model.md
    // §4.5).
    for task in db(project, |db| db.read(|c| tasks_in_phase(c, Phase::Accepting))).await? {
        let id = task.clone();
        let stale = db(project, move |db| {
            db.read(|c| match latest_verification(c, &id)? {
                Some(v) => evidence_is_stale(c, &v),
                None => Ok(true),
            })
        })
        .await?;
        if stale {
            ignore_rejection(runtime(project, Reverify { task_id: task }).await)?;
        }
    }
    if let Some(job) = db(project, |db| db.read(next_preview)).await? {
        spawn_job(project, "preview".into(), results::preview(project.clone(), job));
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
pub(crate) fn ignore_rejection<T>(result: anyhow::Result<T>) -> anyhow::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.downcast_ref::<lobotomy_core::Error>().is_some_and(|e| e.code().is_some()) => Ok(None),
        Err(e) => Err(e),
    }
}
