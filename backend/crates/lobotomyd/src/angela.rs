//! Angela, the user's global manager (data-model.md §10.6; roadmap.md M4). One per host. Her
//! turns, her conversation and her inbox are in a database of her own in the host directory, with
//! the projects' schema and command layer. She has no slot; her working directory is an empty
//! directory of her own. She reads the projects through her MCP tools (`mcp::angela`).
//!
//! She runs when the user writes to her, or when a project settles: nothing in it moves on
//! without the user (manager-actions.md §6 "Manager 什么时候运行").

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lobotomy_core::ledger::ledger_lines;
use lobotomy_core::manager::{Cursor, TellManager, cursors};
use lobotomy_core::task::queued_messages;
use lobotomy_core::turn::{MarkUnfinishedUnknown, RegisterTurn, current_session, unfinished_turn};
use lobotomy_core::view::manager::{ChangesRead, changes_since, project_read, settled};
use lobotomy_core::view::overview;
use lobotomy_harness::{Harness, MCP_SERVER};
use rusqlite::Transaction;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::gui::{self, project_attention};
use crate::host::Host;
use crate::instance::Instance;
use crate::projects::Projects;
use crate::runner::{self, db, launch, runtime};
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
    /// run left (data-model.md §3.3) and starts her scheduler, which watches `projects`.
    pub async fn start(host: Arc<Host>, projects: Arc<Projects>) -> anyhow::Result<Self> {
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
        let scheduler = tokio::spawn(schedule(instance.clone(), projects, scheduling.clone()));
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

/// Her turns: one when messages wait for her and her quota domain is open. She runs only then;
/// nothing runs in the background (manager-actions.md §6).
async fn schedule(instance: Arc<Instance>, projects: Arc<Projects>, shutdown: CancellationToken) {
    loop {
        if let Err(e) = tick(&instance, &projects).await {
            tracing::error!(error = format!("{e:#}"), "Angela's scheduler step failed");
        }
        tokio::select! {
            _ = instance.wake.notified() => {}
            _ = tokio::time::sleep(POLL) => {}
            _ = shutdown.cancelled() => return,
        }
    }
}

async fn tick(instance: &Arc<Instance>, projects: &Projects) -> anyhow::Result<()> {
    reconcile(instance).await?;
    let harness = db(instance, |db| db.read(|c| lobotomy_core::role::load_role(c, ROLE))).await?.harness;
    if instance.host.is_blocked(&harness)? {
        return Ok(());
    }
    tell(instance, projects).await?;
    if let Some(turn) = ignore_rejection(runtime(instance, RegisterTurn { role: ROLE.into() }).await)? {
        launch(instance, turn.turn_id);
    }
    launch_registered(instance).await
}

/// What a settled project waits for: nothing in it moves on without the user (manager-actions.md
/// §6).
#[derive(Clone, Copy, PartialEq)]
enum Settled {
    /// Something waits for the user: "全员在等你".
    Waiting,
    /// Nothing waits and no task is open: "活干完了".
    Done,
}

impl Settled {
    fn label(self) -> &'static str {
        match self {
            Settled::Waiting => "全员在等你",
            Settled::Done => "活干完了",
        }
    }
}

/// One open project as her scheduler sees it.
struct Seen {
    id: String,
    name: String,
    settled: Option<Settled>,
    /// What waits for the user there, in a line each.
    waiting: Vec<String>,
    /// What changed since her cursor; for a project she sees for the first time, its last event,
    /// where her cursor starts.
    changes: Result<ChangesRead, i64>,
}

/// What the runtime tells her about the projects (manager-actions.md §6 "Manager 什么时候运行").
///
/// A settled project wakes her when something she does not know yet happened there since her
/// cursor: a change someone other than her caused. The runtime's own work, such as writing a slot
/// or a preview, is no change to her, so a restart wakes nobody. Whenever she is about to run,
/// what changed in each project goes in her input, and her cursors move past it. A project she sees
/// for the first time starts her cursor at its present. Nothing is told while her turn runs: it
/// comes with her next one.
async fn tell(instance: &Instance, projects: &Projects) -> anyhow::Result<()> {
    let (inbox, running, fresh, cursors) = db(instance, |db| {
        db.read(|c| {
            Ok((
                !queued_messages(c, ROLE)?.is_empty(),
                unfinished_turn(c, ROLE)?.is_some(),
                // Her next turn starts a session: none yet, or a new one without turns.
                match current_session(c, ROLE, None)? {
                    None => true,
                    Some(s) => {
                        !c.query_row("SELECT EXISTS (SELECT 1 FROM turn WHERE native_session_id = ?1)", [&s.id], |r| {
                            r.get::<_, bool>(0)
                        })?
                    }
                },
                cursors(c)?,
            ))
        })
    })
    .await?;
    if running {
        return Ok(());
    }
    let cursors: HashMap<String, Cursor> = cursors.into_iter().map(|c| (c.project_id.clone(), c)).collect();
    let blocked: Vec<String> = Harness::ALL
        .iter()
        .filter(|h| instance.host.is_blocked(h.as_str()).unwrap_or(false))
        .map(|h| h.as_str().to_owned())
        .collect();
    let mut seen = Vec::new();
    for entry in instance.host.db.projects()? {
        let Some(project) = projects.get(&entry.id) else { continue };
        let (blocked, after) = (blocked.clone(), cursors.get(&entry.id).map(|c| c.seq));
        let (still, overview, changes) = db(&project, move |db| {
            db.read(|c| {
                let changes = match after {
                    Some(after) => Ok(changes_since(c, after)?),
                    None => Err(lobotomy_core::view::last_event(c)?),
                };
                Ok((settled(c, &blocked)?, overview(c, MCP_SERVER)?, changes))
            })
        })
        .await?;
        let attention = project_attention(&project, &overview);
        let open = overview.tasks.iter().any(|t| !t.task.phase.is_closed());
        let settled = match (still, attention.is_empty(), open) {
            (false, ..) => None,
            (true, false, _) => Some(Settled::Waiting),
            (true, true, false) => Some(Settled::Done),
            // Paused tasks and nothing else: the user stopped them.
            (true, true, true) => None,
        };
        let waiting = project_read(&overview, &attention).waiting.into_iter().map(|w| w.what).collect();
        seen.push(Seen { id: entry.id, name: entry.name, settled, waiting, changes });
    }

    // A role questioning her interpretation goes to the user unfiltered, in a message of its own in
    // her conversation, and wakes her to correct herself (manager-actions.md §2.3).
    for s in &seen {
        let Ok(changes) = &s.changes else { continue };
        if changes.mismatches.is_empty() {
            continue;
        }
        let body = format!(
            "⚠ {} 中有角色认为 Angela 的解读与用户原话不一致\n\n{}\n\n这条提示由运行时直接放进对话，用户能看到。请对照用户原话修正。",
            s.name,
            changes.mismatches.iter().map(|m| format!("- {m}")).collect::<Vec<_>>().join("\n")
        );
        runtime(instance, TellManager { role: ROLE.into(), body: Some(body), cursors: Vec::new() }).await?;
    }
    let wakes = |s: &Seen| {
        s.changes.as_ref().is_ok_and(|c| !c.mismatches.is_empty() || (s.settled.is_some() && c.by_others > 0))
    };
    let woken: Vec<&Seen> = seen.iter().filter(|s| wakes(s)).collect();
    let mut sections = Vec::new();
    let mut moved = Vec::new();
    for s in &seen {
        let changes = match &s.changes {
            Err(last) => {
                moved.push(Cursor { project_id: s.id.clone(), seq: *last });
                continue;
            }
            // Nothing is told unless she runs now.
            Ok(_) if woken.is_empty() && !inbox => continue,
            Ok(changes) => changes,
        };
        let mut lines = Vec::new();
        if let Some(state) = s.settled.filter(|_| wakes(s)) {
            lines.push(format!("现在{}。", state.label()));
            lines.extend(s.waiting.iter().map(|w| format!("- 等用户：{w}")));
        }
        if !changes.lines.is_empty() {
            lines.push(format!("事件 {} 之后的变化：", changes.after));
            lines.extend(changes.lines.iter().map(|l| format!("- {l}")));
            if changes.not_listed > 0 {
                lines.push(format!("- 另有 {} 条，用 read_events 从事件 {} 往后读", changes.not_listed, changes.after));
            }
        }
        if !lines.is_empty() {
            sections.push(format!("## {}（{}）\n{}", s.name, s.id, lines.join("\n")));
        }
        moved.push(Cursor { project_id: s.id.clone(), seq: changes.as_of });
    }
    // A new session of hers starts from her ledger: what holds, for her to resume from
    // (manager-actions.md §3.4).
    let runs = !woken.is_empty() || inbox;
    let recalled = if runs && fresh { recall(instance, projects).await? } else { None };
    if moved.is_empty() && recalled.is_none() {
        return Ok(());
    }
    let states: Vec<String> = woken
        .iter()
        .filter(|s| s.changes.as_ref().is_ok_and(|c| c.by_others > 0))
        .filter_map(|s| s.settled.map(|state| format!("{} {}", s.name, state.label())))
        .collect();
    let summary = match (states.is_empty(), sections.is_empty()) {
        (false, _) => states.join("；"),
        (true, true) => "你的记录".to_owned(),
        (true, false) => "各项目自上次以来的变化".to_owned(),
    };
    sections.splice(0..0, recalled);
    let body = (!sections.is_empty()).then(|| format!("{summary}\n\n{}", sections.join("\n\n")));
    runtime(instance, TellManager { role: ROLE.into(), body, cursors: moved }).await?;
    Ok(())
}

/// Her ledger in the first input of a new session: the entries that hold, by scope, hypotheses
/// apart (manager-actions.md §3.4). `None` when it is empty.
async fn recall(instance: &Instance, projects: &Projects) -> anyhow::Result<Option<String>> {
    let ledgers = crate::mcp::angela::ledgers(instance, projects, None, true).await?;
    let mut scopes = Vec::new();
    let user = ledger_lines(&ledgers.user);
    if !user.is_empty() {
        scopes.push(format!("用户范围：\n{}", bullets(&user)));
    }
    for p in &ledgers.projects {
        let lines = ledger_lines(&p.entries);
        match &p.unavailable {
            Some(why) => scopes.push(format!("{}（{}）：读不到，{why}", p.name, p.id)),
            None if !lines.is_empty() => scopes.push(format!("{}（{}）：\n{}", p.name, p.id, bullets(&lines))),
            None => {}
        }
    }
    Ok((!scopes.is_empty())
        .then(|| format!("## 你的记录（新会话开始，由运行时放入；用 read_ledger 查看全部）\n{}", scopes.join("\n"))))
}

fn bullets(lines: &[String]) -> String {
    lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")
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
