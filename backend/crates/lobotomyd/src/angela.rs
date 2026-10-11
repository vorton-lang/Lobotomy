//! Angela, the user's global manager (data-model.md §10.6; roadmap.md M4). One per host. Her
//! turns, her conversation and her inbox are in a database of her own in the host directory, with
//! the projects' schema and command layer. She has no slot; her working directory is an empty
//! directory of her own. She reads the projects through her MCP tools (`mcp::angela`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use lobotomy_core::turn::{MarkUnfinishedUnknown, RegisterTurn};
use rusqlite::Transaction;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::gui;
use crate::host::Host;
use crate::instance::Instance;
use crate::runner::{self, launch, runtime};
use crate::scheduler::{ignore_rejection, launch_registered, reconcile};

/// Her instance's id: what the GUI's requests and pushes name instead of a project id, and her
/// segment of the MCP path (frontend.md §3 rule 8; harness-adapter.md §2). Project ids are
/// prefixed ULIDs, so it names no project.
pub const ID: &str = "angela";

/// Her role's name (roles-and-tasks.md §1.2).
pub const ROLE: &str = "Angela";

/// How often her scheduler looks again without being woken, mainly to see whether the CLI of an
/// unknown turn has exited.
const POLL: Duration = Duration::from_secs(5);

/// Angela at work: her instance, her scheduler and the watcher that tells GUIs what changed.
pub struct Angela {
    pub instance: Arc<Instance>,
    scheduling: CancellationToken,
    scheduler: JoinHandle<()>,
    watching: CancellationToken,
    watcher: JoinHandle<()>,
}

impl Angela {
    /// Opens her instance in `<host>/angela`, created on first use, reconciles the turns the last
    /// run left (data-model.md §3.3) and starts her scheduler.
    pub async fn start(host: Arc<Host>) -> anyhow::Result<Self> {
        let dir = host.dir.join("angela");
        let instance = Arc::new(
            tokio::task::spawn_blocking(move || {
                let instance = Instance::open(ID, &dir, host)?;
                instance.db.write(seed)?;
                anyhow::Ok(instance)
            })
            .await??,
        );
        runtime(&instance, MarkUnfinishedUnknown {}).await?;
        let watching = CancellationToken::new();
        let watcher = tokio::spawn(gui::watch(instance.clone(), watching.clone()));
        let scheduling = CancellationToken::new();
        let scheduler = tokio::spawn(schedule(instance.clone(), scheduling.clone()));
        Ok(Self { instance, scheduling, scheduler, watching, watcher })
    }

    /// Normal shutdown (harness-adapter.md §1.8): scheduling stops, and her running CLI is
    /// interrupted and waited for until `deadline`.
    pub async fn stop(self, deadline: Instant) {
        self.scheduling.cancel();
        let _ = self.scheduler.await;
        runner::interrupt_all(&self.instance, deadline).await;
        self.watching.cancel();
        let _ = self.watcher.await;
    }
}

/// Her database has the projects' schema, whose migrations add the projects' roles. It keeps
/// only her: the other roles are removed while nothing refers to them, and she is added once.
fn seed(tx: &Transaction<'_>) -> lobotomy_core::Result<()> {
    tx.execute(
        "DELETE FROM role WHERE kind <> 'manager'
           AND name NOT IN (SELECT role FROM native_session) AND name NOT IN (SELECT role FROM message)
           AND name NOT IN (SELECT executor FROM task) AND name NOT IN (SELECT role FROM thread)",
        [],
    )?;
    tx.execute(
        "INSERT INTO role (name, kind, harness) SELECT ?1, 'manager', 'claude'
         WHERE NOT EXISTS (SELECT 1 FROM role WHERE name = ?1)",
        [ROLE],
    )?;
    Ok(())
}

/// Her turns: one when the user's messages wait for her and her quota domain is open. She runs
/// only then; nothing runs in the background (manager-actions.md §6).
async fn schedule(instance: Arc<Instance>, shutdown: CancellationToken) {
    loop {
        if let Err(e) = tick(&instance).await {
            tracing::error!(error = format!("{e:#}"), "Angela's scheduler step failed");
        }
        tokio::select! {
            _ = instance.wake.notified() => {}
            _ = tokio::time::sleep(POLL) => {}
            _ = shutdown.cancelled() => return,
        }
    }
}

async fn tick(instance: &Arc<Instance>) -> anyhow::Result<()> {
    reconcile(instance).await?;
    let harness = runner::db(instance, |db| db.read(|c| lobotomy_core::role::load_role(c, ROLE))).await?.harness;
    if instance.host.is_blocked(&harness)? {
        return Ok(());
    }
    if let Some(turn) = ignore_rejection(runtime(instance, RegisterTurn { role: ROLE.into() }).await)? {
        launch(instance, turn.turn_id);
    }
    launch_registered(instance).await
}

#[cfg(test)]
mod tests {
    use lobotomy_core::Db;
    use lobotomy_core::role::list_roles;

    use super::*;

    #[test]
    fn her_database_holds_only_her_role() {
        let db = Db::open_in_memory().unwrap();
        db.write(seed).unwrap();
        db.write(seed).unwrap();
        let roles = db.read(list_roles).unwrap();
        let names: Vec<_> = roles.iter().map(|r| (r.name.as_str(), r.kind.as_str(), r.slot.is_none())).collect();
        assert_eq!(names, [(ROLE, "manager", true)]);
    }
}
