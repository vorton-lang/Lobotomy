//! Runs one registered turn: starts the CLI, delivers the input, stores the transcript items and
//! ends the turn (harness-adapter.md §1; data-model.md §3, §7).

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use futures::FutureExt as _;
use lobotomy_core::id::now_ms;
use lobotomy_core::item::{NewItem, externalize, record_item};
use lobotomy_core::role::{Role, load_role};
use lobotomy_core::turn::{
    EndTurn, Failure, FailureKind, InputDelivered, Outcome, SessionIdentified, Turn, TurnLaunched, load_turn,
};
use lobotomy_core::workspace::load_workspace;
use lobotomy_core::{Caller, Command, Db, Error};
use lobotomy_harness::event::{Event, Item, ItemKind};
use lobotomy_harness::process::{self, Spawned};
use lobotomy_harness::{Harness, MCP_SERVER, Output, claude, codex};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::instance::{Instance, LiveItem, LiveTurn, RunningTurn};
use crate::launch;
use crate::mcp::command_id_in;

const WORKER_INSTRUCTIONS: &str = include_str!("prompts/worker.md");
const MANAGER_INSTRUCTIONS: &str = include_str!("prompts/manager.md");

/// Runs a db call on the blocking pool.
pub async fn db<T: Send + 'static>(
    instance: &Instance,
    f: impl FnOnce(&Db) -> lobotomy_core::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let db = instance.db.clone();
    Ok(tokio::task::spawn_blocking(move || f(&db)).await??)
}

/// Runs a runtime command on the blocking pool.
pub async fn runtime<C>(instance: &Instance, cmd: C) -> anyhow::Result<C::Output>
where
    C: Command + Send + 'static,
    C::Output: Send + 'static,
{
    db(instance, move |db| db.execute(&Caller::Runtime, &cmd)).await
}

/// Turns a panic in `f` into an error, so the bookkeeping after a runner or a job still happens:
/// otherwise a panicked turn stays in `running` for good, and shutdown waits its whole grace
/// period for it (#16).
pub(crate) async fn catch_panic<T>(f: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    match AssertUnwindSafe(f).catch_unwind().await {
        Ok(result) => result,
        Err(panic) => {
            let what = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            anyhow::bail!("panicked: {what}")
        }
    }
}

/// Starts a runner for a registered turn unless one is running already.
pub fn launch(instance: &Arc<Instance>, turn_id: String) {
    let stop = CancellationToken::new();
    let mut running = instance.running.lock().unwrap();
    if running.contains_key(&turn_id) {
        return;
    }
    running
        .insert(turn_id.clone(), RunningTurn { pid: None, interrupt_requested: false, stop: stop.clone(), task: None });
    let id = turn_id.clone();
    let runner = instance.tasks.spawn({
        let instance = instance.clone();
        async move {
            let result = catch_panic(run(&instance, &turn_id, stop)).await;
            if let Err(e) = &result {
                tracing::error!(turn_id, error = format!("{e:#}"), "turn runner failed");
                // The turn must not stay running without a runner. If the CLI may still be alive,
                // the turn becomes unknown at the next start and is reconciled then.
                let end = EndTurn {
                    turn_id: turn_id.clone(),
                    outcome: Outcome::Failed,
                    failure: Some(Failure::new(FailureKind::Other, format!("运行时出错：{e:#}"))),
                };
                if let Err(e) = runtime(&instance, end).await {
                    tracing::error!(turn_id, error = format!("{e:#}"), "could not end the turn");
                }
            }
            instance.running.lock().unwrap().remove(&turn_id);
            instance.wake.notify_one();
            result
        }
    });
    // Publish the task with its stop handle, so archiving cannot take an unjoinable runner.
    running.get_mut(&id).unwrap().task = Some(runner);
}

/// Asks the turn's CLI to stop (harness-adapter.md §1.3 rule 6). The turn ends as interrupted
/// once the CLI exits.
pub async fn interrupt(instance: &Instance, turn_id: &str) -> anyhow::Result<()> {
    let pid = {
        let mut running = instance.running.lock().unwrap();
        let turn =
            running.get_mut(turn_id).ok_or_else(|| Error::rejected("not_running", "the turn is not running here"))?;
        turn.interrupt_requested = true;
        turn.pid.ok_or_else(|| Error::rejected("not_started", "the CLI has not started yet"))?
    };
    process::interrupt(pid, &instance.host.harness.interrupt_helper).await?;
    Ok(())
}

/// Interrupts every turn the instance runs and waits for them until `deadline`, as quitting does
/// (harness-adapter.md §1.8).
pub async fn interrupt_all(instance: &Instance, deadline: Instant) {
    let running: Vec<String> = instance.running.lock().unwrap().keys().cloned().collect();
    for turn_id in running {
        if let Err(e) = interrupt(instance, &turn_id).await {
            tracing::warn!(instance = instance.id, turn_id, error = format!("{e:#}"), "could not interrupt");
        }
    }
    while !instance.running.lock().unwrap().is_empty() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The role's instructions, by its kind (roles-and-tasks.md §1.1).
fn instructions(role: &Role) -> String {
    let text = if role.kind == "manager" { MANAGER_INSTRUCTIONS } else { WORKER_INSTRUCTIONS };
    text.replace("{name}", &role.name)
}

/// Where the turn's CLI runs: its slot (harness-adapter.md §3). The manager has no slot; her
/// working directory is an empty directory of her own (data-model.md §10.6).
async fn working_dir(instance: &Instance, turn: &Turn, role: &Role) -> anyhow::Result<PathBuf> {
    match turn.workspace_id.clone() {
        Some(id) => Ok(instance.slot_dir(&db(instance, move |db| db.read(|c| load_workspace(c, &id))).await?.name)),
        None if role.kind == "manager" => {
            let dir = instance.data_dir.join("work");
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            Ok(dir)
        }
        None => anyhow::bail!("{} has no slot", role.name),
    }
}

/// What the runner saw in the CLI's output.
#[derive(Default)]
struct Observed {
    /// Any line on stdout. Codex writes `thread.started` first, before it takes the input.
    output: bool,
    completed: bool,
    failed: Option<String>,
    /// The harness refused the turn for its quota (data-model.md §8.3).
    quota: Option<Failure>,
    last_error: Option<String>,
    unparsed: bool,
    /// An event could not be recorded; the raw output is kept to replay it (#16).
    unrecorded: bool,
    /// The harness's session id could not be recorded. Going on would start the next turn in a
    /// fresh session without anyone noticing, so the turn fails and the role waits (#16).
    session_unrecorded: Option<String>,
}

/// A file that belongs to one turn and goes with it, such as Claude's MCP configuration, which holds
/// the turn's token.
struct TurnFile(std::path::PathBuf);

impl Drop for TurnFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A failure before the CLI wrote anything: its input reached no session.
fn unstarted(kind: FailureKind, message: String) -> Option<Failure> {
    Some(Failure { unstarted: true, ..Failure::new(kind, message) })
}

async fn end(instance: &Instance, turn_id: &str, outcome: Outcome, failure: Option<Failure>) -> anyhow::Result<()> {
    // A quota rejection blocks the whole domain, not just this role (data-model.md §8.3). Block it
    // before the turn ends: whoever sees the ended turn, the scheduler included, sees the block.
    if let Some(failure) = failure.as_ref().filter(|f| f.kind == FailureKind::Quota) {
        let id = turn_id.to_owned();
        let harness = db(instance, move |db| db.read(|c| load_turn(c, &id))).await?.harness;
        if instance.host.db.block(&harness, failure.resets_at, &failure.message, now_ms())? {
            tracing::warn!(harness, message = failure.message, "quota domain blocked");
        }
    }
    runtime(instance, EndTurn { turn_id: turn_id.to_owned(), outcome, failure }).await?;
    Ok(())
}

async fn run(instance: &Arc<Instance>, turn_id: &str, stop: CancellationToken) -> anyhow::Result<()> {
    let id = turn_id.to_owned();
    let turn = db(instance, move |db| db.read(|c| load_turn(c, &id))).await?;
    let role_name = turn.role.clone();
    let role = db(instance, move |db| db.read(|c| load_role(c, &role_name))).await?;
    // The turn's session decides the harness: a role that changed its harness gets a new session.
    let Some(harness) = Harness::parse(&turn.harness) else {
        let message = format!("{} 使用 {}，这个版本的 Lobotomy 不支持", role.name, turn.harness);
        return end(instance, turn_id, Outcome::Failed, unstarted(FailureKind::Other, message)).await;
    };

    // A slot was ready when the turn was registered (harness-adapter.md §3).
    let cwd = working_dir(instance, &turn, &role).await?;
    let turns_dir = instance.data_dir.join("turns");
    std::fs::create_dir_all(&turns_dir).with_context(|| format!("creating {}", turns_dir.display()))?;
    let cli = instance.host.harness.cli(harness);
    let permission = instance.host.permission(harness)?;
    let mcp_url = instance.mcp_url(&turn.token);
    // The user's choice for the role on this harness (harness-adapter.md §1.2); tests set an
    // effort on the CLI for roles that have none.
    let effort = role.effort.clone().or_else(|| cli.reasoning_effort.clone());
    let (args, _mcp_config) = match harness {
        Harness::Claude => {
            // Named after the turn, which tells its CLI apart on Linux (data-model.md §3.3).
            let path = turns_dir.join(format!("{turn_id}.mcp.json"));
            std::fs::write(&path, claude::mcp_config(&mcp_url))
                .with_context(|| format!("writing {}", path.display()))?;
            let session = match &turn.native_id {
                Some(id) => claude::Session::Resume(id.clone()),
                None => claude::Session::New(claude::new_session_id()),
            };
            let args = claude::TurnArgs {
                session,
                model: role.model.clone(),
                effort,
                permission,
                instructions: instructions(&role),
                mcp_config: path.clone(),
            };
            (args.to_args(), Some(TurnFile(path)))
        }
        Harness::Codex => {
            let args = codex::TurnArgs {
                resume: turn.native_id.clone(),
                model: role.model.clone(),
                reasoning_effort: effort,
                permission,
                developer_instructions: instructions(&role),
                mcp_url,
                disabled_skills: codex::user_skills(),
            };
            (args.to_args(), None)
        }
    };
    let (program, prefix) = cli.program()?;
    let args: Vec<String> = prefix.iter().cloned().chain(args).collect();
    let what = launch::What::Program { program, args: &args };
    let mut spawned = match launch::spawn(what, &cwd, &instance.empty_gh_config_dir()).await {
        Ok(spawned) => spawned,
        Err(e) => {
            let message = format!("无法启动 {}（{}）：{e:#}", harness.label(), program.display());
            return end(instance, turn_id, Outcome::Failed, unstarted(FailureKind::Other, message)).await;
        }
    };
    if let Some(running) = instance.running.lock().unwrap().get_mut(turn_id) {
        running.pid = Some(spawned.pid);
    }
    // Recorded before the CLI runs: after a crash, a turn without a pid never ran
    // (data-model.md §3.3).
    let launched =
        TurnLaunched { turn_id: turn_id.to_owned(), pid: spawned.pid as i64, process_start: spawned.process_start };
    if let Err(e) = runtime(instance, launched).await {
        spawned.finish().await?;
        return Err(e);
    }
    let live = LiveTurn { role: turn.role.clone(), started_at: now_ms(), items: Default::default() };
    instance.update_live(|turns| {
        turns.insert(turn_id.to_owned(), live);
    });
    let outcome = tokio::select! {
        biased;
        _ = stop.cancelled() => None,
        result = catch_panic(drive(instance, harness, &turn, &mut spawned)) => Some(result),
    };
    instance.update_live(|turns| {
        turns.remove(turn_id);
    });
    // Whatever happened, the CLI is gone or must go before the turn ends.
    spawned.finish().await?;
    // Archiving reconciles a forced stop when the project opens again, quitting when the backend
    // starts again. The CLI is already gone.
    let Some(outcome) = outcome else { return Ok(()) };
    let (outcome, failure, clean) = outcome?;
    end(instance, turn_id, outcome, failure).await?;
    if clean {
        for ext in ["jsonl", "stderr"] {
            let _ = std::fs::remove_file(instance.raw_output_path(turn_id, ext));
        }
    }
    Ok(())
}

/// Lets the CLI run to its end. Returns the outcome, the failure, and whether the raw output can
/// be deleted (data-model.md §7.4).
async fn drive(
    instance: &Arc<Instance>,
    harness: Harness,
    turn: &Turn,
    spawned: &mut Spawned,
) -> anyhow::Result<(Outcome, Option<Failure>, bool)> {
    let stderr_path = instance.raw_output_path(&turn.id, "stderr");
    let io = launch::run(spawned, turn.input.as_bytes(), |stderr| async move {
        let mut file = tokio::fs::File::create(stderr_path).await?;
        tokio::io::copy(&mut BufReader::new(stderr), &mut file).await
    })
    .await?;
    let stderr_copy = io.stderr;
    match io.input {
        Ok(()) => {
            runtime(instance, InputDelivered { turn_id: turn.id.clone() }).await?;
            store(instance, turn, None, "input", json!({ "source": "turn.input" })).await?;
        }
        // The CLI exited early; its output says why.
        Err(e) => tracing::warn!(turn_id = turn.id, error = %e, "could not deliver the input"),
    }

    let mut raw = tokio::fs::File::create(instance.raw_output_path(&turn.id, "jsonl")).await?;
    let mut lines = BufReader::new(io.stdout).lines();
    let mut seen = Observed::default();
    let mut output = Output::new(harness);
    while let Some(line) = lines.next_line().await? {
        raw.write_all(line.as_bytes()).await?;
        raw.write_all(b"\n").await?;
        seen.output = true;
        for event in output.parse_line(&line) {
            if let Err(e) = observe(instance, turn, event, &mut seen).await {
                tracing::warn!(turn_id = turn.id, error = format!("{e:#}"), "could not record an event");
                seen.unrecorded = true;
            }
        }
    }
    raw.flush().await?;
    let status = spawned.child.wait().await?;
    let _ = stderr_copy.await;

    let interrupted = instance.running.lock().unwrap().get(&turn.id).is_some_and(|t| t.interrupt_requested);
    let result = if let Some(error) = seen.session_unrecorded {
        let path = instance.raw_output_path(&turn.id, "jsonl");
        let message = format!(
            "Lobotomy 没能记下这一轮的 {} 会话 ID：{error}\n接着用原来的会话继续，或新建会话。原始输出保留在 {}",
            harness.label(),
            path.display()
        );
        (Outcome::Failed, Some(Failure::new(FailureKind::Other, message)))
    } else if let Some(failure) = seen.quota {
        (Outcome::Failed, Some(failure))
    } else if seen.completed {
        (Outcome::Completed, None)
    } else if interrupted {
        // The user asked for it. Codex stops without a turn end event; Claude ends the turn with
        // an error, which is the interruption, not a failure of its own.
        (Outcome::Interrupted, None)
    } else if let Some(message) = seen.failed {
        // Codex's quota rejections are not known yet; they count as ordinary failures
        // (data-model.md §8.3).
        (Outcome::Failed, Some(Failure::new(FailureKind::Other, message)))
    } else if let Some(message) = seen.last_error {
        (Outcome::Failed, Some(Failure::new(FailureKind::Other, message)))
    } else {
        // The CLI stopped on its own without saying why on stdout, as when it rejects its
        // arguments at start (data-model.md §3.2, #13).
        let status = match status.code() {
            Some(code) => format!("退出码 {code}"),
            None => status.to_string(),
        };
        let path = instance.raw_output_path(&turn.id, "stderr");
        let mut message = format!("{} 自行退出（{status}），没有报告 turn 结束。", harness.label());
        let mut kind = FailureKind::Other;
        match stderr_excerpt(&path, &turn.token).await {
            Ok(excerpt) if !excerpt.is_empty() => {
                let refused = match harness {
                    Harness::Claude => claude::permission_refused(&excerpt),
                    Harness::Codex => codex::permission_refused(&excerpt),
                };
                if refused {
                    kind = FailureKind::Permission;
                }
                message.push_str(&format!("\nstderr 开头：\n{excerpt}\n完整内容见 {}", path.display()))
            }
            Ok(_) => message.push_str("stderr 为空。"),
            Err(e) => message.push_str(&format!("读不到 stderr（{}）：{e}", path.display())),
        }
        (Outcome::Failed, Some(Failure { unstarted: !seen.output, ..Failure::new(kind, message) }))
    };
    let clean = result.0 == Outcome::Completed && !seen.unparsed && !seen.unrecorded;
    Ok((result.0, result.1, clean))
}

const EXCERPT_LINES: usize = 20;
const EXCERPT_BYTES: u64 = 4096;

/// The start of what the CLI wrote to stderr, where an error comes before its backtrace. The
/// turn's token is in the CLI's arguments, so an error that quotes them would show it.
async fn stderr_excerpt(path: &Path, token: &str) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt as _;
    let mut head = Vec::new();
    tokio::fs::File::open(path).await?.take(EXCERPT_BYTES).read_to_end(&mut head).await?;
    let text = String::from_utf8_lossy(&head).replace(token, "<token>");
    Ok(text.trim_end().lines().take(EXCERPT_LINES).collect::<Vec<_>>().join("\n"))
}

async fn observe(instance: &Arc<Instance>, turn: &Turn, event: Event, seen: &mut Observed) -> anyhow::Result<()> {
    match event {
        Event::SessionStarted { native_id } => {
            if turn.native_id.as_deref() != Some(native_id.as_str()) {
                let identified = runtime(instance, SessionIdentified { turn_id: turn.id.clone(), native_id }).await;
                if let Err(e) = &identified {
                    seen.session_unrecorded = Some(format!("{e:#}"));
                }
                identified?;
            }
        }
        Event::ItemCompleted(item) => {
            instance.update_live(|turns| {
                if let Some(live) = turns.get_mut(&turn.id) {
                    live.items.remove(&item.native_id);
                }
            });
            store_harness_item(instance, turn, item).await?
        }
        Event::TurnCompleted { .. } => seen.completed = true,
        Event::TurnFailed { message } => seen.failed = Some(message),
        // A harness may say it more than once, the reset time only in some of them.
        Event::QuotaRejected { resets_at, message } => {
            let resets_at = resets_at.or(seen.quota.as_ref().and_then(|q| q.resets_at));
            seen.quota = Some(Failure { resets_at, ..Failure::new(FailureKind::Quota, message) });
        }
        Event::Error { message } => {
            store(instance, turn, None, "error", json!({ "message": message })).await?;
            seen.last_error = Some(message);
        }
        Event::Unknown(_) | Event::Unparsed(_) => seen.unparsed = true,
        // An item in progress is shown live and never stored (frontend.md §3).
        Event::ItemStarted(item) | Event::ItemUpdated(item) => instance.update_live(|turns| {
            if let Some(live) = turns.get_mut(&turn.id) {
                let started_at = live.items.get(&item.native_id).map_or_else(now_ms, |i| i.started_at);
                let entry = LiveItem { kind: item.kind.as_str().to_owned(), content: item.content, started_at };
                live.items.insert(item.native_id, entry);
            }
        }),
        Event::TurnStarted => {}
    }
    Ok(())
}

/// Stores a completed harness item. A call to a Lobotomy tool keeps only a reference to its
/// command record, which already holds the arguments (data-model.md §7.2). A reply that names a
/// command of another instance, as Angela's reads of a project's events do, is kept whole.
async fn store_harness_item(instance: &Arc<Instance>, turn: &Turn, item: Item) -> anyhow::Result<()> {
    let mut content = item.content;
    let mut command_id = None;
    if item.kind == ItemKind::McpCall && content["server"] == MCP_SERVER {
        let reply = content["result_text"].as_str().unwrap_or_default();
        let named = command_id_in(reply).map(str::to_owned);
        let recorded = match named {
            Some(id) => db(instance, move |db| db.read(|c| Ok(command_recorded(c, &id)?.then_some(id)))).await?,
            None => None,
        };
        if let Some(id) = recorded {
            command_id = Some(id);
            if let Value::Object(fields) = &mut content {
                for field in ["arguments", "result", "result_text"] {
                    fields.remove(field);
                }
            }
        }
    }
    store_with(instance, turn, Some(item.native_id), item.kind.as_str(), content, command_id).await
}

fn command_recorded(conn: &rusqlite::Connection, id: &str) -> lobotomy_core::Result<bool> {
    let found = conn.query_row("SELECT 1 FROM command_record WHERE id = ?1", [id], |_| Ok(()));
    match found {
        Ok(()) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

async fn store(
    instance: &Arc<Instance>,
    turn: &Turn,
    native_id: Option<String>,
    kind: &str,
    content: Value,
) -> anyhow::Result<()> {
    store_with(instance, turn, native_id, kind, content, None).await
}

async fn store_with(
    instance: &Arc<Instance>,
    turn: &Turn,
    native_id: Option<String>,
    kind: &str,
    mut content: Value,
    command_id: Option<String>,
) -> anyhow::Result<()> {
    let (instance, role, turn_id, kind) = (instance.clone(), turn.role.clone(), turn.id.clone(), kind.to_owned());
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let blobs = externalize(&instance.blobs, &mut content);
        let item = NewItem {
            role: &role,
            turn_id: &turn_id,
            native_item_id: native_id.as_deref(),
            kind: &kind,
            content: &content,
            command_id: command_id.as_deref(),
        };
        instance.db.write(|tx| record_item(tx, &item, &blobs, now_ms()))?;
        Ok(())
    })
    .await?
}
