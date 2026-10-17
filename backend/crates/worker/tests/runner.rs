//! The agent driver inside the worker host: how a turn's CLI fails, stops and is recorded
//! (harness-adapter.md; data-model.md §3, §7). These moved here from the backend's tests when the
//! backend stopped running executors itself (tech-plan.md §7 step 1).

mod common;

use std::collections::BTreeMap;
use std::path::Path;
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

/// Stopping an attempt whose CLI ignores the interrupt ends the CLI and its children once the grace
/// period is over; the attempt is then canceled (harness-adapter.md §1.8).
#[tokio::test]
async fn a_cli_that_ignores_the_interrupt_ends_with_its_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("deaf-cli.mjs");
    let ready = dir.path().join("tree-ready");
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
    let codex = serde_json::to_string(&["node", &script.to_string_lossy()]).unwrap();
    let mut worker = Worker::start_with(&root(&dir), &["--stop-grace", "1"], &[("LOBOTOMY_CODEX", &codex)]).await;
    worker.subscribe(BTreeMap::new()).await;
    worker.call::<CreateTask>(&create("t", "deaf")).await.unwrap();
    worker.state("t", AttemptState::Working).await;
    let deadline = Instant::now() + WAIT;
    let child: u32 = loop {
        // The handlers and the ordinary child are ready before the interrupt; a launched pid alone
        // could let SIGINT end Node before it installed its handler and hide the forced stop.
        if let Some(pid) = std::fs::read_to_string(&ready).ok().and_then(|s| s.parse().ok()) {
            break pid;
        }
        assert!(Instant::now() < deadline, "the process tree never got ready");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(child_is_running(child));

    worker.call::<Stop>(&StopParams { task_id: "t".into(), level: StopLevel::Attempt }).await.unwrap();
    worker.state("t", AttemptState::Canceled).await;
    assert!(!child_is_running(child), "the ordinary child ended with the tree");
    worker.quit().await;
}

#[tokio::test]
async fn an_exited_cli_reaps_children_holding_output_pipes_and_keeps_its_final_events() {
    for keep_stdout in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let root = root(&dir);
        let script = dir.path().join("exiting-cli.mjs");
        let pid_file = dir.path().join("child-pid");
        std::fs::write(
            &script,
            format!(
                r#"import {{ spawn }} from 'node:child_process';
import fs from 'node:fs';
for await (const chunk of process.stdin) {{}}
const child = spawn(process.execPath, ['-e', 'setInterval(() => {{}}, 1000)'], {{
  stdio: ['ignore', {}, 'inherit'],
}});
child.unref();
fs.writeFileSync({}, String(child.pid));
const events = [
  {{ type: 'thread.started', thread_id: 'exiting-cli-session' }},
  {{ type: 'turn.started' }},
  {{ type: 'item.completed', item: {{ id: 'last', type: 'agent_message', text: 'the final reply' }} }},
  {{ type: 'turn.completed', usage: {{ input_tokens: 1, output_tokens: 1 }} }},
];
await new Promise(resolve => process.stdout.write(events.map(event => JSON.stringify(event)).join('\n') + '\n', resolve));
process.exit(0);
"#,
                if keep_stdout { "'inherit'" } else { "'ignore'" },
                serde_json::to_string(&pid_file).unwrap(),
            ),
        )
        .unwrap();
        let codex = serde_json::to_string(&["node", &script.to_string_lossy()]).unwrap();
        let mut worker = Worker::start_with(&root, &["--stop-grace", "1"], &[("LOBOTOMY_CODEX", &codex)]).await;
        worker.subscribe(BTreeMap::new()).await;
        worker.call::<CreateTask>(&create("t", "finish normally")).await.unwrap();
        let ended = tokio::time::timeout(
            WAIT,
            worker.state_why("t", AttemptState::Blocked, |reason| reason.contains("without reporting")),
        )
        .await;
        if ended.is_err() {
            // Clean up the otherwise stranded descendant before reporting the regression.
            worker.call::<Stop>(&StopParams { task_id: "t".into(), level: StopLevel::Attempt }).await.unwrap();
            worker.state("t", AttemptState::Canceled).await;
            worker.quit().await;
            panic!("the exited CLI left its attempt running (stdout inherited: {keep_stdout})");
        }
        let child: u32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        assert!(!child_is_running(child), "the descendant still holds the output pipes");
        let db = rusqlite::Connection::open_with_flags(
            root.join("tasks/t/lobotomy.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let outcome: String = db.query_row("SELECT outcome FROM turn", [], |row| row.get(0)).unwrap();
        assert_eq!(outcome, "completed");
        let reply: String = db
            .query_row("SELECT json_extract(content, '$.text') FROM item WHERE kind = 'agent_message'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(reply, "the final reply");
        worker.quit().await;
    }
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
