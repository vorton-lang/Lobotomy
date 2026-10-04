//! The backend end to end with a fake Codex (tests/fixtures/fake-codex.mjs, run by Node): the
//! scheduler, the turn runner, the MCP service and the database together. Needs `node` on PATH.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use lobotomy_core::id::now_ms;
use lobotomy_core::task::{Abandon, CreateTask, SendMessage, StartAttempt, load_task, open_attempt};
use lobotomy_core::turn::{Continue, EndTurn, Failure, FailureKind, Outcome, RegisterTurn, TurnState};
use lobotomy_core::{Caller, Db};
use lobotomyd::Backend;
use lobotomyd::host::Host;

#[tokio::test]
async fn a_task_runs_to_done_through_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    let task = create_task(&db, "FAKE:done");

    let turn = ended_turn(&db).await;
    assert_eq!(turn.outcome, Some(Outcome::Completed));
    assert!(turn.done_at.is_some(), "org_report(done) reached the turn");
    assert!(turn.native_id.is_some(), "thread.started identified the session");
    let attempt = db.read(|c| open_attempt(c, &task)).unwrap().unwrap();
    assert_eq!(attempt.done_turn_id.as_deref(), Some(turn.id.as_str()));
    assert_eq!(std::fs::read_to_string(slot(dir.path()).join("work.txt")).unwrap(), "hi");

    // The role instructions and the per-turn MCP URL were passed.
    let args = std::fs::read_to_string(slot(dir.path()).join("last-args.json")).unwrap();
    assert!(args.contains("你是 Malkuth") && args.contains(&turn.token), "{args}");

    // Items: the input, two messages and the tool call, which refers to its command record.
    let items: Vec<(String, Option<String>, String)> = db
        .read(|c| {
            let mut stmt = c.prepare("SELECT kind, command_id, content FROM item WHERE turn_id = ?1 ORDER BY seq")?;
            Ok(stmt.query_map([&turn.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    let kinds: Vec<&str> = items.iter().map(|(kind, ..)| kind.as_str()).collect();
    assert_eq!(kinds, ["input", "agent_message", "mcp_call", "agent_message"]);
    let (_, command_id, content) = &items[2];
    assert!(command_id.as_deref().is_some_and(|id| id.starts_with("cmd_")));
    assert!(!content.contains("arguments"), "the call's arguments live in the command record only: {content}");

    // A clean turn leaves no raw output behind.
    assert!(!backend.project.raw_output_path(&turn.id, "jsonl").exists());

    // After done the role waits for the capture; a new message does not start a turn.
    db.execute(
        &Caller::User,
        &SendMessage { request_id: "m1".into(), role: "Malkuth".into(), task_id: None, body: "还有一件事".into() },
    )
    .unwrap();
    backend.project.wake.notify_one();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(last(&db).unwrap().id, turn.id);
    backend.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_failed_turn_holds_the_role_until_continue() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    create_task(&db, "FAKE:fail");

    let failed = ended_turn(&db).await;
    assert_eq!(failed.outcome, Some(Outcome::Failed));
    assert_eq!(failed.failure.as_ref().map(|f| f.message.as_str()), Some("boom"));
    assert!(backend.project.raw_output_path(&failed.id, "jsonl").exists(), "an abnormal turn keeps its raw output");

    db.execute(
        &Caller::User,
        &SendMessage { request_id: "m1".into(), role: "Malkuth".into(), task_id: None, body: "换个做法".into() },
    )
    .unwrap();
    backend.project.wake.notify_one();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(last(&db).unwrap().id, failed.id, "held: no automatic turn after a failure");

    db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: "Malkuth".into() }).unwrap();
    backend.project.wake.notify_one();
    let next = wait_for("the continued turn", || {
        last(&db).filter(|t| t.id != failed.id && t.state == TurnState::Ended)
    })
    .await;
    assert_eq!(next.outcome, Some(Outcome::Completed));
    assert_eq!(next.native_id, failed.native_id, "continue resumes the same native session");
    assert!(next.input.contains("失败") && next.input.contains("换个做法"), "{}", next.input);
    backend.shutdown(Duration::from_secs(5)).await;
}

#[cfg(windows)]
#[tokio::test]
async fn interrupting_a_turn_ends_it_as_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    create_task(&db, "FAKE:sleep");

    let turn = wait_for("the turn to run", || last(&db).filter(|t| t.state == TurnState::Running)).await;
    // Give the fake time to install its SIGINT handler.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    lobotomyd::runner::interrupt(&backend.project, &turn.id).await.unwrap();
    let ended = ended_turn(&db).await;
    assert_eq!(ended.outcome, Some(Outcome::Interrupted));
    let pid = ended.pid.unwrap() as u32;
    assert!(!lobotomy_harness::process::is_running(pid, ended.process_start.unwrap()));
    backend.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_turn_left_registered_by_the_last_run_is_reconciled_as_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    {
        // The last run registered a turn and stopped before starting the CLI.
        let project = open_project(dir.path(), fake_codex());
        create_task(&project.db, "FAKE:done");
        project.db.execute(&Caller::Runtime, &StartAttempt { task_id: next_task(&project.db) }).unwrap();
        project.db.execute(&Caller::Runtime, &RegisterTurn { role: "Malkuth".into() }).unwrap();
    }
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    let turn = ended_turn(&db).await;
    assert_eq!(turn.outcome, Some(Outcome::Interrupted));
    assert_eq!(turn.pid, None, "the CLI never ran");
    backend.shutdown(Duration::from_secs(5)).await;
}

/// When the blob file cannot be written, the large field stays in the database: the item is
/// stored complete and the turn ends cleanly (#10).
#[tokio::test]
async fn a_large_field_is_stored_even_when_its_blob_cannot_be_written() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("project")).unwrap();
    std::fs::write(dir.path().join("project").join("blobs"), "not a directory").unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    create_task(&db, "FAKE:big");

    let turn = ended_turn(&db).await;
    assert_eq!(turn.outcome, Some(Outcome::Completed));
    let output: String = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT json_extract(content, '$.output') FROM item WHERE turn_id = ?1 AND kind = 'command'",
                [&turn.id],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(output.len(), 200 * 1024);
    assert!(!backend.project.raw_output_path(&turn.id, "jsonl").exists(), "nothing is missing, so the raw output goes");
    backend.shutdown(Duration::from_secs(5)).await;
}

/// One real Codex turn on the user's subscription: the role writes a file and reports done.
#[tokio::test]
#[ignore = "runs a real Codex turn"]
async fn a_real_codex_turn_reports_done() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Backend::start(Arc::new(open_project(dir.path(), real_codex())), 0).await.unwrap();
    let db = backend.project.db.clone();
    db.execute(
        &Caller::User,
        &CreateTask {
            request_id: "c1".into(),
            title: "创建 hello.txt".into(),
            body: "在工作目录中创建 hello.txt，内容只有一行：hi".into(),
            criteria: "hello.txt 存在，内容为 hi".into(),
            executor: "Malkuth".into(),
        },
    )
    .unwrap();
    let turn = ended_turn(&db).await;
    let items: Vec<(String, Option<String>)> = db
        .read(|c| {
            let mut stmt = c.prepare("SELECT kind, command_id FROM item WHERE turn_id = ?1 ORDER BY seq")?;
            Ok(stmt.query_map([&turn.id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    eprintln!("outcome {:?}, failure {:?}, items {items:?}", turn.outcome, turn.failure);
    // Role sessions persist so they can resume; this one is a test, so keep it out of the
    // user's Codex history.
    if let Some(session) = &turn.native_id {
        let deleted = std::process::Command::new(&backend.project.host.harness.codex[0])
            .args(["delete", "--force", session])
            .output()
            .map(|out| out.status.success());
        eprintln!("deleted Codex session {session}: {deleted:?}");
    }
    assert_eq!(turn.outcome, Some(Outcome::Completed));
    assert!(turn.done_at.is_some(), "the role reported done");
    assert!(items.iter().any(|(kind, id)| kind == "mcp_call" && id.is_some()), "the call refers to its command record");
    let hello = std::fs::read_to_string(slot(dir.path()).join("hello.txt")).unwrap();
    assert_eq!(hello.trim(), "hi");
    backend.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_blocked_quota_domain_starts_nothing_until_a_retry_passes() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    let host = backend.project.host.clone();
    // Unknown reset time: no automatic check is planned (data-model.md §8.4).
    host.db.block("codex", None, "limit", now_ms()).unwrap();
    let task = create_task(&db, "FAKE:done");
    settle(&backend).await;
    assert!(last(&db).is_none(), "no turn while the domain is blocked");
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().phase, lobotomy_core::task::Phase::Queued);

    // The user's retry runs a check (the fake completes it) and opens the domain.
    let domain = host.retry("codex").await.unwrap();
    assert!(!domain.is_blocked());
    backend.project.wake.notify_one();
    let turn = ended_turn(&db).await;
    assert_eq!(turn.outcome, Some(Outcome::Completed));
    backend.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn after_recovery_a_quota_failed_role_waits_for_the_user() {
    let dir = tempfile::tempdir().unwrap();
    {
        // The last turn failed for quota and blocked the domain.
        let project = open_project(dir.path(), fake_codex());
        let task = create_task(&project.db, "FAKE:done");
        project.db.execute(&Caller::Runtime, &StartAttempt { task_id: task }).unwrap();
        let turn = project.db.execute(&Caller::Runtime, &RegisterTurn { role: "Malkuth".into() }).unwrap();
        let failure = Failure { kind: FailureKind::Quota, message: "limit".into(), resets_at: None };
        let end = EndTurn { turn_id: turn.turn_id, outcome: Outcome::Failed, failure: Some(failure) };
        project.db.execute(&Caller::Runtime, &end).unwrap();
        project.host.db.block("codex", None, "limit", now_ms()).unwrap();
    }
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    let failed = last(&db).unwrap();

    // The user's retry opens the domain; the runtime does not continue the role on its own.
    assert!(!backend.project.host.retry("codex").await.unwrap().is_blocked());
    settle(&backend).await;
    assert_eq!(last(&db).unwrap().id, failed.id, "no automatic continue (data-model.md §8.5)");

    db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: "Malkuth".into() }).unwrap();
    backend.project.wake.notify_one();
    let next = wait_for("the continued turn", || {
        last(&db).filter(|t| t.id != failed.id && t.state == TurnState::Ended)
    })
    .await;
    assert!(next.input.contains("额度不足"), "{}", next.input);
    backend.shutdown(Duration::from_secs(5)).await;
}

/// After a failed turn, abandoning the task lets the next task run, in a fresh session (#11).
#[tokio::test]
async fn abandoning_a_failed_task_lets_the_next_one_run() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    let a = create_task(&db, "FAKE:fail");
    let failed = ended_turn(&db).await;
    assert_eq!(failed.outcome, Some(Outcome::Failed));
    let b = create_task(&db, "FAKE:done");
    db.execute(&Caller::User, &Abandon { request_id: "x1".into(), task_id: a, reason: String::new() }).unwrap();
    backend.project.wake.notify_one();

    let next = wait_for("B's turn", || last(&db).filter(|t| t.id != failed.id && t.state == TurnState::Ended)).await;
    assert_eq!(next.task_id.as_deref(), Some(b.as_str()));
    assert_eq!(next.outcome, Some(Outcome::Completed));
    assert_ne!(next.native_id, failed.native_id, "B runs in a fresh harness session");
    backend.shutdown(Duration::from_secs(5)).await;
}

/// The quota check call against the real Codex: `--ephemeral`, outside every role session.
#[tokio::test]
#[ignore = "runs a real Codex call"]
async fn a_real_quota_check_passes() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::open(dir.path(), real_codex()).unwrap());
    host.db.block("codex", None, "test", now_ms()).unwrap();
    assert!(!host.retry("codex").await.unwrap().is_blocked());
}

/// A check whose CLI writes more stderr than a pipe holds still finishes, and the next retry
/// runs a new check (#10).
#[tokio::test]
async fn a_quota_check_reads_stderr_and_can_run_again() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::open(dir.path(), fake_codex_with(&["--fake-stderr-flood"])).unwrap());
    for _ in 0..2 {
        host.db.block("codex", None, "test", now_ms()).unwrap();
        assert!(!host.retry("codex").await.unwrap().is_blocked());
    }
}

/// A check that does not finish times out, leaves the domain blocked, and does not keep later
/// retries from running (#10).
#[tokio::test]
async fn a_hanging_quota_check_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = fake_codex_with(&["--fake-hang"]);
    harness.probe_timeout = Duration::from_secs(2);
    let host = Arc::new(Host::open(dir.path(), harness).unwrap());
    host.db.block("codex", None, "test", now_ms()).unwrap();
    for _ in 0..2 {
        let started = Instant::now();
        let domain = host.retry("codex").await.unwrap();
        assert!(domain.is_blocked(), "a timed-out check leaves the domain blocked");
        assert!(started.elapsed() >= Duration::from_secs(2), "the retry ran a check of its own");
    }
}

fn next_task(db: &Db) -> String {
    db.read(|c| lobotomy_core::task::next_queued_task(c, "Malkuth")).unwrap().unwrap().id
}
