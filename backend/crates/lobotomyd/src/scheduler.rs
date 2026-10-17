//! A project's scheduler: it gives the queued tasks of the project's workers to them
//! (tech-plan.md §7 step 1, group 7). The backend no longer runs executors itself; turns a project
//! still has from before are reconciled once their CLI has exited (data-model.md §3.3).

use std::sync::Arc;
use std::time::Duration;

use lobotomy_agent::runner::{db, reconcile};
use lobotomy_core::project::load_project;
use tokio_util::sync::CancellationToken;

use crate::project::Project;

/// How often the scheduler looks again without being woken: a worker host that was not running
/// may be by now.
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
    if let Some(workers) = project.host.workers.get() {
        workers.dispatch(project).await?;
    }
    Ok(())
}
