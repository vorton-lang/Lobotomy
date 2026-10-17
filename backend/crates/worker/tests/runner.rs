//! The agent driver inside the worker host: how a turn's CLI fails, stops and is recorded
//! (harness-adapter.md; data-model.md §3, §7). These moved here from the backend's tests when the
//! backend stopped running executors itself (tech-plan.md §7 step 1).

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use lobotomy_worker_protocol::event::AttemptState;
use lobotomy_worker_protocol::ops::{CreateTask, SendMessage, Stop, StopLevel, StopParams};

/// The raw output files the task's turns left, by extension.
fn raw_outputs(root: &Path, task: &str, ext: &str) -> Vec<std::path::PathBuf> {
    let turns = root.join("tasks").join(task).join("turns");
    std::fs::read_dir(turns)
        .map(|entries| {
            entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == ext)).collect()
        })
        .unwrap_or_default()
}

/// A CLI that exits on its own with only stderr to show holds the attempt, and the reason says
/// why, without the turn's token; the raw stderr stays (#13).
#[tokio::test]
async fn a_cli_that_exits_with_only_stderr_holds_the_attempt_with_its_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let codex = fixture("fake-codex.mjs", &["--fake-start-error"]);
    let mut worker = Worker::start_with(&root, &[], &[("LOBOTOMY_CODEX", &codex)]).await;
    worker.subscribe(BTreeMap::new()).await;
    worker.call::<CreateTask>(&create("t", "FAKE:done")).await.unwrap();
    let reason = worker.state_why("t", AttemptState::Blocked, |r| r.contains("退出码 1")).await;
    assert!(reason.contains("Error: mcp_servers.lobotomy.url"), "{reason}");
    assert!(reason.contains("<token>") && !reason.contains("tok_"), "the token is left out: {reason}");
    assert!(reason.contains("  7: <unknown>"), "the excerpt keeps the start of stderr: {reason}");
    assert!(!raw_outputs(&root, "t", "stderr").is_empty(), "the raw stderr stays as evidence");
    worker.quit().await;
}

/// A turn whose session id cannot be recorded fails, and its raw output stays: otherwise the next
/// turn would quietly start a fresh session and lose the agent's context (#16).
#[tokio::test]
async fn a_session_id_that_cannot_be_recorded_holds_the_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let mut worker = Worker::start(&root).await;
    worker.subscribe(BTreeMap::new()).await;
    worker.call::<CreateTask>(&create("t", "先说一句")).await.unwrap();
    worker.state_why("t", AttemptState::Blocked, |r| r.contains("without reporting")).await;
    worker.call::<SendMessage>(&message("t", "m1", "FAKE:newthread")).await.unwrap();
    let reason = worker.state_why("t", AttemptState::Blocked, |r| r.contains("会话 ID")).await;
    assert!(reason.contains("没能记下这一轮的 Codex 会话 ID"), "{reason}");
    assert!(!raw_outputs(&root, "t", "jsonl").is_empty(), "the raw output stays");
    worker.quit().await;
}

/// When the blob file cannot be written, the large field stays in the database: the item is stored
/// complete, the turn ends cleanly and its raw output goes (#10).
#[tokio::test]
async fn a_large_field_is_stored_even_when_its_blob_cannot_be_written() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let task_dir = root.join("tasks").join("t");
    std::fs::create_dir_all(&task_dir).unwrap();
    std::fs::write(task_dir.join("blobs"), "not a directory").unwrap();
    let mut worker = Worker::start(&root).await;
    worker.subscribe(BTreeMap::new()).await;
    worker.call::<CreateTask>(&create("t", "FAKE:big FAKE:done")).await.unwrap();
    worker.state("t", AttemptState::Delivered).await;
    let db =
        rusqlite::Connection::open_with_flags(task_dir.join("lobotomy.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let output: String = db
        .query_row("SELECT json_extract(content, '$.output') FROM item WHERE kind = 'command'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(output.len(), 200 * 1024);
    assert!(raw_outputs(&root, "t", "jsonl").is_empty(), "nothing is missing, so the raw output went");
    worker.quit().await;
}

/// A turn that ends without the agent reporting done or asking holds the attempt; a message makes
/// it go on (#13; worker-interface.md §10).
#[tokio::test]
async fn a_turn_that_ends_without_a_report_waits_for_a_message() {
    let dir = tempfile::tempdir().unwrap();
    let mut worker = Worker::start(&root(&dir)).await;
    worker.subscribe(BTreeMap::new()).await;
    worker.call::<CreateTask>(&create("t", "FAKE:refused")).await.unwrap();
    worker.state_why("t", AttemptState::Blocked, |r| r.contains("without reporting done or asking")).await;
    worker.call::<SendMessage>(&message("t", "m1", "FAKE:done")).await.unwrap();
    worker.state("t", AttemptState::Delivered).await;
    worker.quit().await;
}

/// A harness that refuses for its quota holds the attempt, and the worker host tells the
/// orchestrator, with the reset time the harness gave (data-model.md §8.3).
#[tokio::test]
async fn a_refused_quota_holds_the_attempt_and_tells_the_orchestrator() {
    let dir = tempfile::tempdir().unwrap();
    let mut worker = Worker::start_on(&root(&dir), "claude", &[], &[]).await;
    worker.subscribe(BTreeMap::new()).await;
    worker.call::<CreateTask>(&create("t", "FAKE:quota")).await.unwrap();
    let reason = worker.state_why("t", AttemptState::Blocked, |r| r.contains("session limit")).await;
    assert!(!reason.is_empty());
    let resources = worker.resources_until(|r| r.quota.iter().any(|q| q.blocked)).await;
    let quota = resources.quota.iter().find(|q| q.harness == "claude").unwrap();
    assert_eq!(quota.resets_at, Some(1_790_868_000_000));
    worker.quit().await;
}

/// A CLI that ignores the interrupt, with an ordinary child, for `LOBOTOMY_CODEX`; and the file
/// where it writes the child's pid once the tree is ready (`deaf_child`).
fn deaf_cli(dir: &Path) -> (String, PathBuf) {
    let script = dir.join("deaf-cli.mjs");
    let ready = dir.join("tree-ready");
    std::fs::write(
        &script,
        format!(
            r#"import {{ fork }} from 'node:child_process';
import fs from 'node:fs';
process.on('SIGINT', () => {{}});
if (process.argv[2] === 'child') {{
  process.send(process.pid);
  setInterval(() => {{}}, 1000);
}} else {{
  const child = fork(process.argv[1], ['child'], {{ stdio: ['ignore', 'ignore', 'ignore', 'ipc'] }});
  child.once('message', pid => fs.writeFileSync({}, String(pid)));
  for await (const chunk of process.stdin) {{}}
  setInterval(() => {{}}, 1000);
}}
"#,
            serde_json::to_string(&ready).unwrap()
        ),
    )
    .unwrap();
    (serde_json::to_string(&["node", &script.to_string_lossy()]).unwrap(), ready)
}

/// The deaf CLI's child, once its handlers and the child are ready: a launched pid alone could let
/// SIGINT end Node before it installed its handler and hide the forced stop.
async fn deaf_child(ready: &Path) -> u32 {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(pid) = std::fs::read_to_string(ready).ok().and_then(|s| s.parse().ok()) {
            assert!(child_is_running(pid));
            return pid;
        }
        assert!(Instant::now() < deadline, "the process tree never got ready");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Stopping an attempt whose CLI ignores the interrupt ends the CLI and its children once the grace
/// period is over; the attempt is then canceled (harness-adapter.md §1.8).
#[tokio::test]
async fn a_cli_that_ignores_the_interrupt_ends_with_its_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let (codex, ready) = deaf_cli(dir.path());
    let mut worker = Worker::start_with(&root(&dir), &["--stop-grace", "1"], &[("LOBOTOMY_CODEX", &codex)]).await;
    worker.subscribe(BTreeMap::new()).await;
    worker.call::<CreateTask>(&create("t", "deaf")).await.unwrap();
    worker.state("t", AttemptState::Working).await;
    let child = deaf_child(&ready).await;

    worker.call::<Stop>(&StopParams { task_id: "t".into(), level: StopLevel::Attempt }).await.unwrap();
    worker.state("t", AttemptState::Canceled).await;
    assert!(!child_is_running(child), "the ordinary child ended with the tree");
    worker.quit().await;
}

/// Quitting does not wait for the CLI to answer an interrupt: the CLI and its children end at once.
/// In the first real acceptance, Codex ignored Ctrl+C for the whole wait and the app looked stuck
/// (harness-adapter.md §1.8).
#[tokio::test]
async fn quitting_ends_a_cli_that_ignores_the_interrupt_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let (codex, ready) = deaf_cli(dir.path());
    let mut worker = Worker::start_with(&root(&dir), &[], &[("LOBOTOMY_CODEX", &codex)]).await;
    worker.subscribe(BTreeMap::new()).await;
    worker.call::<CreateTask>(&create("t", "deaf")).await.unwrap();
    worker.state("t", AttemptState::Working).await;
    let child = deaf_child(&ready).await;

    let quitting = Instant::now();
    worker.quit().await;
    assert!(quitting.elapsed() < Duration::from_secs(5), "quitting took {:?}", quitting.elapsed());
    assert!(!child_is_running(child), "the ordinary child ended with the tree");
}

fn child_is_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        stat.rsplit_once(')').is_some_and(|(_, rest)| !matches!(rest.split_whitespace().next(), Some("Z" | "X")))
    }
    #[cfg(windows)]
    {
        let out =
            std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    }
}
