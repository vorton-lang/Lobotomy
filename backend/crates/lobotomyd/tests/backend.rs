//! The backend end to end with a fake Codex (tests/fixtures/fake-codex.mjs, run by Node): the
//! scheduler, the turn runner, the MCP service and the database together. Needs `node` on PATH.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lobotomy_core::task::{CreateTask, SendMessage, open_attempt};
use lobotomy_core::turn::{Continue, Outcome, Turn, TurnState, last_turn};
use lobotomy_core::{Caller, Db};
use lobotomyd::Backend;
use lobotomyd::project::{HarnessConfig, Project};

fn fake_codex() -> HarnessConfig {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("fake-codex.mjs");
    HarnessConfig {
        codex: vec!["node".into(), script.to_string_lossy().into_owned()],
        interrupt_helper: vec![env!("CARGO_BIN_EXE_lobotomyd").into(), "ctrl-c".into()],
        codex_reasoning_effort: None,
    }
}

async fn start(dir: &Path) -> Backend {
    let project = Arc::new(Project::open(dir, fake_codex()).unwrap());
    Backend::start(project, 0).await.unwrap()
}

fn create_task(db: &Db, body: &str) -> String {
    db.execute(
        &Caller::User,
        &CreateTask {
            request_id: format!("c-{body}"),
            title: "测试任务".into(),
            body: body.into(),
            criteria: "work.txt 存在".into(),
            executor: "Malkuth".into(),
        },
    )
    .unwrap()
    .id
}

fn last(db: &Db) -> Option<Turn> {
    db.read(|c| last_turn(c, "Malkuth")).unwrap()
}

async fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn ended_turn(db: &Db) -> Turn {
    wait_for("the turn to end", || last(db).filter(|t| t.state == TurnState::Ended)).await
}

fn slot(dir: &Path) -> PathBuf {
    dir.join("slots").join("worker")
}

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
        let project = Project::open(dir.path(), fake_codex()).unwrap();
        create_task(&project.db, "FAKE:done");
        project.db.execute(&Caller::Runtime, &lobotomy_core::task::StartAttempt { task_id: next_task(&project.db) }).unwrap();
        project.db.execute(&Caller::Runtime, &lobotomy_core::turn::RegisterTurn { role: "Malkuth".into() }).unwrap();
    }
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    let turn = ended_turn(&db).await;
    assert_eq!(turn.outcome, Some(Outcome::Interrupted));
    assert_eq!(turn.pid, None, "the CLI never ran");
    backend.shutdown(Duration::from_secs(5)).await;
}

/// One real Codex turn on the user's subscription: the role writes a file and reports done.
#[tokio::test]
#[ignore = "runs a real Codex turn"]
async fn a_real_codex_turn_reports_done() {
    let dir = tempfile::tempdir().unwrap();
    let harness = HarnessConfig {
        codex: vec![lobotomy_harness::codex::locate().to_string_lossy().into_owned()],
        interrupt_helper: vec![env!("CARGO_BIN_EXE_lobotomyd").into(), "ctrl-c".into()],
        codex_reasoning_effort: Some("low".into()),
    };
    let backend = Backend::start(Arc::new(Project::open(dir.path(), harness).unwrap()), 0).await.unwrap();
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
        let deleted = std::process::Command::new(&backend.project.harness.codex[0])
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

fn next_task(db: &Db) -> String {
    db.read(|c| lobotomy_core::task::next_queued_task(c, "Malkuth")).unwrap().unwrap().id
}
