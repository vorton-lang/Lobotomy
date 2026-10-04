//! Runs one registered turn: starts the CLI, delivers the input, stores the transcript items and
//! ends the turn (harness-adapter.md §1; data-model.md §3, §7).

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use lobotomy_core::id::now_ms;
use lobotomy_core::item::{NewItem, externalize, record_item};
use lobotomy_core::role::{Role, load_role};
use lobotomy_core::turn::{
    EndTurn, Failure, FailureKind, InputDelivered, Outcome, SessionIdentified, Turn, TurnLaunched, load_turn,
};
use lobotomy_core::{Caller, Command, Db};
use lobotomy_harness::codex::{self, TurnArgs};
use lobotomy_harness::event::{Event, Item, ItemKind};
use lobotomy_harness::process::{self, Spec, Spawned};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::mcp::command_id_in;
use crate::project::{Project, RunningTurn};

const WORKER_INSTRUCTIONS: &str = include_str!("prompts/worker.md");

/// Runs a db call on the blocking pool.
pub async fn db<T: Send + 'static>(
    project: &Arc<Project>,
    f: impl FnOnce(&Db) -> lobotomy_core::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let db = project.db.clone();
    Ok(tokio::task::spawn_blocking(move || f(&db)).await??)
}

/// Runs a runtime command on the blocking pool.
pub async fn runtime<C>(project: &Arc<Project>, cmd: C) -> anyhow::Result<C::Output>
where
    C: Command + Send + 'static,
    C::Output: Send + 'static,
{
    db(project, move |db| db.execute(&Caller::Runtime, &cmd)).await
}

/// Starts a runner for a registered turn unless one is running already.
pub fn launch(project: &Arc<Project>, turn_id: String) {
    {
        let mut running = project.running.lock().unwrap();
        if running.contains_key(&turn_id) {
            return;
        }
        running.insert(turn_id.clone(), RunningTurn { pid: None, interrupt_requested: false });
    }
    let project = project.clone();
    tokio::spawn(async move {
        if let Err(e) = run(&project, &turn_id).await {
            tracing::error!(turn_id, error = format!("{e:#}"), "turn runner failed");
            // The turn must not stay running without a runner. If the CLI may still be alive,
            // the turn becomes unknown at the next start and is reconciled then.
            let end = EndTurn {
                turn_id: turn_id.clone(),
                outcome: Outcome::Failed,
                failure: Some(Failure { kind: FailureKind::Other, message: format!("运行时出错：{e:#}"), resets_at: None }),
            };
            if let Err(e) = runtime(&project, end).await {
                tracing::error!(turn_id, error = format!("{e:#}"), "could not end the turn");
            }
        }
        project.running.lock().unwrap().remove(&turn_id);
        project.wake.notify_one();
    });
}

/// Asks the turn's CLI to stop (harness-adapter.md §1.3 rule 6). The turn ends as interrupted
/// once the CLI exits.
pub async fn interrupt(project: &Arc<Project>, turn_id: &str) -> anyhow::Result<()> {
    let pid = {
        let mut running = project.running.lock().unwrap();
        let turn = running.get_mut(turn_id).context("the turn is not running here")?;
        turn.interrupt_requested = true;
        turn.pid.context("the CLI has not started yet")?
    };
    process::interrupt(pid, &project.host.harness.interrupt_helper).await?;
    Ok(())
}

fn instructions(role: &Role) -> String {
    WORKER_INSTRUCTIONS.replace("{name}", &role.name)
}

/// Process environment that trims what the CLI can do outside the project
/// (harness-adapter.md §1.6): `git push` fails locally and `gh` is logged out.
fn capability_env(gh_config_dir: &Path) -> Vec<(String, String)> {
    let mut env = vec![("GIT_CONFIG_COUNT".to_owned(), "3".to_owned())];
    for (i, prefix) in ["https://", "git@", "ssh://"].iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{i}"), "url.lobotomy-push-disabled://.pushInsteadOf".to_owned()));
        env.push((format!("GIT_CONFIG_VALUE_{i}"), (*prefix).to_owned()));
    }
    env.push(("GH_CONFIG_DIR".to_owned(), gh_config_dir.to_string_lossy().into_owned()));
    env
}

/// What the runner saw in the CLI's output.
#[derive(Default)]
struct Observed {
    completed: bool,
    failed: Option<String>,
    last_error: Option<String>,
    unparsed: bool,
}

async fn end(project: &Arc<Project>, turn_id: &str, outcome: Outcome, failure: Option<(FailureKind, String)>) -> anyhow::Result<()> {
    let failure = failure.map(|(kind, message)| Failure { kind, message, resets_at: None });
    let quota = failure.clone().filter(|f| f.kind == FailureKind::Quota);
    runtime(project, EndTurn { turn_id: turn_id.to_owned(), outcome, failure }).await?;
    // A quota rejection blocks the whole domain, not just this role (data-model.md §8.3).
    if let Some(failure) = quota {
        let id = turn_id.to_owned();
        let harness = db(project, move |db| db.read(|c| load_turn(c, &id))).await?.harness;
        if project.host.db.block(&harness, failure.resets_at, &failure.message, now_ms())? {
            tracing::warn!(harness, message = failure.message, "quota domain blocked");
        }
    }
    Ok(())
}

async fn run(project: &Arc<Project>, turn_id: &str) -> anyhow::Result<()> {
    let id = turn_id.to_owned();
    let turn = db(project, move |db| db.read(|c| load_turn(c, &id))).await?;
    let role_name = turn.role.clone();
    let role = db(project, move |db| db.read(|c| load_role(c, &role_name))).await?;
    if role.harness != "codex" {
        let message = format!("{} 使用 {}，M1 只支持 Codex", role.name, role.harness);
        return end(project, turn_id, Outcome::Failed, Some((FailureKind::Other, message))).await;
    }

    let cwd = project.slot_dir(role.slot.as_deref().unwrap_or(&role.name));
    let gh = project.empty_gh_config_dir();
    for dir in [&cwd, &gh, &project.data_dir.join("turns")] {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let turn_args = TurnArgs {
        resume: turn.native_id.clone(),
        model: role.model.clone(),
        reasoning_effort: project.host.harness.codex_reasoning_effort.clone(),
        developer_instructions: instructions(&role),
        mcp_url: project.mcp_url(&turn.token),
    };
    let (program, prefix) = project.host.harness.codex.split_first().context("no Codex program configured")?;
    let args: Vec<String> = prefix.iter().cloned().chain(turn_args.to_args()).collect();
    let env = capability_env(&gh);
    let spec = Spec { program: Path::new(program), args: &args, cwd: &cwd, env: &env };
    let mut spawned = match process::spawn(&spec) {
        Ok(spawned) => spawned,
        Err(e) => {
            let message = format!("无法启动 Codex（{program}）：{e}");
            return end(project, turn_id, Outcome::Failed, Some((FailureKind::Other, message))).await;
        }
    };
    if let Some(running) = project.running.lock().unwrap().get_mut(turn_id) {
        running.pid = Some(spawned.pid);
    }
    // Recorded before the CLI runs: after a crash, a turn without a pid never ran
    // (data-model.md §3.3).
    let launched = TurnLaunched { turn_id: turn_id.to_owned(), pid: spawned.pid as i64, process_start: spawned.process_start };
    if let Err(e) = runtime(project, launched).await {
        let _ = spawned.child.start_kill();
        return Err(e);
    }
    let outcome = drive(project, &turn, &mut spawned).await;
    // Whatever happened, the CLI is gone or must go before the turn ends.
    let _ = spawned.child.start_kill();
    let _ = spawned.child.wait().await;
    spawned.reap();
    let (outcome, failure, clean) = outcome?;
    end(project, turn_id, outcome, failure).await?;
    if clean {
        for ext in ["jsonl", "stderr"] {
            let _ = std::fs::remove_file(project.raw_output_path(turn_id, ext));
        }
    }
    Ok(())
}

/// Lets the CLI run to its end. Returns the outcome, the failure, and whether the raw output can
/// be deleted (data-model.md §7.4).
async fn drive(
    project: &Arc<Project>,
    turn: &Turn,
    spawned: &mut Spawned,
) -> anyhow::Result<(Outcome, Option<(FailureKind, String)>, bool)> {
    spawned.resume()?;
    let mut stdin = spawned.child.stdin.take().context("no stdin")?;
    let written = async {
        stdin.write_all(turn.input.as_bytes()).await?;
        stdin.shutdown().await
    }
    .await;
    drop(stdin);
    match written {
        Ok(()) => {
            runtime(project, InputDelivered { turn_id: turn.id.clone() }).await?;
            store(project, turn, None, "input", json!({ "source": "turn.input" })).await?;
        }
        // The CLI exited early; its output says why.
        Err(e) => tracing::warn!(turn_id = turn.id, error = %e, "could not deliver the input"),
    }

    let stderr = spawned.child.stderr.take().context("no stderr")?;
    let stderr_path = project.raw_output_path(&turn.id, "stderr");
    let stderr_copy = tokio::spawn(async move {
        let mut file = tokio::fs::File::create(stderr_path).await?;
        tokio::io::copy(&mut BufReader::new(stderr), &mut file).await
    });

    let stdout = spawned.child.stdout.take().context("no stdout")?;
    let mut raw = tokio::fs::File::create(project.raw_output_path(&turn.id, "jsonl")).await?;
    let mut lines = BufReader::new(stdout).lines();
    let mut seen = Observed::default();
    while let Some(line) = lines.next_line().await? {
        raw.write_all(line.as_bytes()).await?;
        raw.write_all(b"\n").await?;
        let Some(event) = codex::parse_line(&line) else { continue };
        if let Err(e) = observe(project, turn, event, &mut seen).await {
            tracing::warn!(turn_id = turn.id, error = format!("{e:#}"), "could not record an event");
        }
    }
    raw.flush().await?;
    let status = spawned.child.wait().await?;
    let _ = stderr_copy.await;

    let interrupted = project.running.lock().unwrap().get(&turn.id).is_some_and(|t| t.interrupt_requested);
    let result = if seen.completed {
        (Outcome::Completed, None)
    } else if let Some(message) = seen.failed {
        // Codex's quota rejections are not known yet; they count as ordinary failures
        // (data-model.md §8.3).
        (Outcome::Failed, Some((FailureKind::Other, message)))
    } else if interrupted {
        (Outcome::Interrupted, None)
    } else if let (false, Some(message)) = (status.success(), seen.last_error) {
        (Outcome::Failed, Some((FailureKind::Other, message)))
    } else {
        (Outcome::Interrupted, None)
    };
    let clean = result.0 == Outcome::Completed && !seen.unparsed;
    Ok((result.0, result.1, clean))
}

async fn observe(project: &Arc<Project>, turn: &Turn, event: Event, seen: &mut Observed) -> anyhow::Result<()> {
    match event {
        Event::SessionStarted { native_id } => {
            if turn.native_id.as_deref() != Some(native_id.as_str()) {
                runtime(project, SessionIdentified { turn_id: turn.id.clone(), native_id }).await?;
            }
        }
        Event::ItemCompleted(item) => store_harness_item(project, turn, item).await?,
        Event::TurnCompleted { .. } => seen.completed = true,
        Event::TurnFailed { message } => seen.failed = Some(message),
        Event::Error { message } => {
            store(project, turn, None, "error", json!({ "message": message })).await?;
            seen.last_error = Some(message);
        }
        Event::Unknown(_) | Event::Unparsed(_) => seen.unparsed = true,
        // Live progress goes to the GUI with increment 4.
        Event::TurnStarted | Event::ItemStarted(_) | Event::ItemUpdated(_) => {}
    }
    Ok(())
}

/// Stores a completed harness item. A call to a Lobotomy tool keeps only a reference to its
/// command record, which already holds the arguments (data-model.md §7.2).
async fn store_harness_item(project: &Arc<Project>, turn: &Turn, item: Item) -> anyhow::Result<()> {
    let mut content = item.content;
    let mut command_id = None;
    if item.kind == ItemKind::McpCall && content["server"] == "lobotomy" {
        let reply = content["result"]["content"][0]["text"].as_str().unwrap_or_default();
        if let Some(id) = command_id_in(reply) {
            command_id = Some(id.to_owned());
            if let Value::Object(fields) = &mut content {
                fields.remove("arguments");
                fields.remove("result");
            }
        }
    }
    store_with(project, turn, Some(item.native_id), item.kind.as_str(), content, command_id).await
}

async fn store(project: &Arc<Project>, turn: &Turn, native_id: Option<String>, kind: &str, content: Value) -> anyhow::Result<()> {
    store_with(project, turn, native_id, kind, content, None).await
}

async fn store_with(
    project: &Arc<Project>,
    turn: &Turn,
    native_id: Option<String>,
    kind: &str,
    mut content: Value,
    command_id: Option<String>,
) -> anyhow::Result<()> {
    let (project, role, turn_id, kind) = (project.clone(), turn.role.clone(), turn.id.clone(), kind.to_owned());
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let blobs = externalize(&project.blobs, &mut content)?;
        let item = NewItem {
            role: &role,
            turn_id: &turn_id,
            native_item_id: native_id.as_deref(),
            kind: &kind,
            content: &content,
            command_id: command_id.as_deref(),
        };
        project.db.write(|tx| record_item(tx, &item, &blobs, now_ms()))?;
        Ok(())
    })
    .await?
}
