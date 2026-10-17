//! The scheduler: reconciles unknown turns, starts attempts and registers turns (data-model.md
//! §3.3, §9.2 runtime commands).
//!
//! Each step is a runtime command whose preconditions live in the command itself; a rejection
//! just means there is nothing to do for that role now.

use std::sync::Arc;
use std::time::Duration;

use lobotomy_agent::ignore_rejection;
use lobotomy_agent::runner::{db, launch, launch_registered, reconcile, runtime};
use lobotomy_core::project::load_project;
use lobotomy_core::role::list_roles;
use lobotomy_core::task::{StartAttempt, next_queued_task, occupant};
use lobotomy_core::turn::RegisterTurn;
use tokio_util::sync::CancellationToken;

use crate::project::Project;

/// How often the scheduler looks again without being woken, mainly to see whether the CLI of an
/// unknown turn has exited.
const POLL: Duration = Duration::from_secs(5);

/// Call [`lobotomy_agent::runner::recover`] once before.
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
