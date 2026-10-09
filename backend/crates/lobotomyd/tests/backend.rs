//! The backend end to end with a fake Codex (tests/fixtures/fake-codex.mjs, run by Node): the
//! scheduler, the turn runner, the MCP service and the database together. Needs `node` on PATH.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use lobotomy_core::capture::stopped_capture;
use lobotomy_core::id::now_ms;
use lobotomy_core::project::{Check, EditProjectConfig, ProjectConfig, current_config, load_project};
use lobotomy_core::task::{Abandon, CreateTask, Phase, SendMessage, load_task, open_attempt};
use lobotomy_core::turn::{Continue, EndTurn, Failure, FailureKind, Outcome, RegisterTurn, TurnState, hold};
use lobotomy_core::verify::{Accept, RetryPreview, VerificationState, latest_verification, preview_stopped};
use lobotomy_core::{Caller, Db};
use lobotomyd::host::Host;
use lobotomyd::project::Project;
use lobotomyd::results::{failures, retry_failed};

/// The M1 chain: the user creates a task, Malkuth works in its slot and reports done, the runtime
/// captures and verifies the candidate, the user accepts it, and the user's repository is
/// fast-forwarded to the new integration version.
#[tokio::test]
async fn a_task_runs_from_done_to_the_users_repository() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    let task = create_task(&db, "FAKE:done");

    let turn = ended_turn(&db).await;
    assert_eq!(turn.outcome, Some(Outcome::Completed));
    assert!(turn.done_at.is_some(), "org_report(done) reached the turn");
    assert!(turn.native_id.is_some(), "thread.started identified the session");
    assert_eq!(std::fs::read_to_string(slot(dir.path()).join("work.txt")).unwrap(), "hi");
    // The slot is a git clone at the integration version: the agent's git view shows its work.
    let repo = user_repo(dir.path());
    let base = git(&repo, &["rev-parse", "HEAD"]);
    assert_eq!(git(&slot(dir.path()), &["rev-parse", "HEAD"]), base);
    assert_eq!(git(&slot(dir.path()), &["status", "--porcelain"]), "?? work.txt");

    // The role instructions and the per-turn MCP URL were passed.
    let args = std::fs::read_to_string(diag(&slot(dir.path())).join("last-args.json")).unwrap();
    assert!(args.contains("你是 Malkuth") && args.contains(&turn.token), "{args}");

    // Items: the input, two messages and the tool call, which refers to its command record.
    let items: Vec<(String, Option<String>, String)> = db
        .read(|c| {
            let mut stmt = c.prepare("SELECT kind, command_id, content FROM item WHERE turn_id = ?1 ORDER BY seq")?;
            Ok(stmt
                .query_map([&turn.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    let kinds: Vec<&str> = items.iter().map(|(kind, ..)| kind.as_str()).collect();
    assert_eq!(kinds, ["input", "agent_message", "mcp_call", "agent_message"]);
    let (_, command_id, content) = &items[2];
    assert!(command_id.as_deref().is_some_and(|id| id.starts_with("cmd_")));
    assert!(!content.contains("arguments"), "the call's arguments live in the command record only: {content}");

    // A clean turn leaves no raw output behind. The runner deletes it after ending the turn.
    let raw = backend.project.raw_output_path(&turn.id, "jsonl");
    wait_for("the raw output to go", || (!raw.exists()).then_some(())).await;

    // The capture became the candidate and passed verification.
    phase(&db, &task, Phase::Accepting).await;
    assert_eq!(db.read(|c| open_attempt(c, &task)).unwrap(), None, "the attempt ended with its candidate");
    let v = db.read(|c| latest_verification(c, &task)).unwrap().unwrap();
    assert_eq!(v.state, VerificationState::Passed);
    assert_eq!(v.base, base);

    // While the task waits for acceptance, messages to the executor wait too.
    db.execute(
        &Caller::User,
        &SendMessage { request_id: "m1".into(), role: "Malkuth".into(), task_id: None, body: "还有一件事".into() },
    )
    .unwrap();
    settle(&backend).await;
    assert_eq!(last(&db).unwrap().id, turn.id);

    let accept = Accept {
        request_id: "a1".into(),
        task_id: task.clone(),
        verification_id: v.id.clone(),
        criteria_version: 1,
        expected_integration: base.clone(),
        dropping: vec![],
    };
    let published = db.execute(&Caller::User, &accept).unwrap();
    backend.project.wake.notify_one();
    wait_for("the preview", || (git(&repo, &["rev-parse", "HEAD"]) == published).then_some(())).await;
    assert_eq!(std::fs::read_to_string(repo.join("work.txt")).unwrap(), "hi");
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
    assert_eq!(git(&repo, &["log", "-1", "--format=%an <%ae>"]), "Test User <user@example.com>");
    let message = git(&repo, &["log", "-1", "--format=%B"]);
    assert!(message.starts_with("测试任务\n\n完成\n\n写了 work.txt"), "{message}");
    assert!(
        message.contains(&format!("Lobotomy-Task: {task}")) && message.contains("Lobotomy-Role: Malkuth"),
        "{message}"
    );
    assert_eq!(git(&repo, &["rev-parse", "HEAD~1"]), base, "one commit per accepted task");
    wait_for("the preview to be recorded", || {
        (db.read(load_project).unwrap().unwrap().previewed == published).then_some(())
    })
    .await;

    // The task closed; the queued message now reaches the idle role, whose slot followed the new
    // integration version.
    phase(&db, &task, Phase::Done).await;
    let next = wait_for("the next turn", || last(&db).filter(|t| t.id != turn.id && t.state == TurnState::Ended)).await;
    assert_eq!(next.task_id, None);
    assert_eq!(git(&slot(dir.path()), &["rev-parse", "HEAD"]), published);
    assert_eq!(git(&slot(dir.path()), &["status", "--porcelain"]), "");
    backend.shutdown(Duration::from_secs(5)).await;
}

fn set_config(db: &Db, change: impl FnOnce(&mut ProjectConfig)) {
    let (version, mut config) = db.read(current_config).unwrap();
    change(&mut config);
    db.execute(
        &Caller::User,
        &EditProjectConfig { request_id: format!("cfg-{version}"), expected_version: version, config },
    )
    .unwrap();
}

/// Checks run in the verification site on the candidate; a failing one sends it straight back to
/// the executor with the output (harness-adapter.md §4.2).
#[tokio::test]
async fn checks_run_on_the_candidate_and_a_failure_goes_back_to_the_executor() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    set_config(&db, |config| {
        config.checks = vec![
            Check {
                command: r#"node -e "process.exit(require('fs').existsSync('work.txt') ? 0 : 1)""#.into(),
                timeout_secs: 60,
            },
            Check { command: r#"node -e "console.log('two tests failed'); process.exit(3)""#.into(), timeout_secs: 60 },
        ];
    });
    let task = create_task(&db, "FAKE:done");
    let first = ended_turn(&db).await;

    // The first check saw the candidate's file; the second failed, and its output went back.
    let next = wait_for("the turn after the failed check", || {
        last(&db).filter(|t| t.id != first.id && t.state == TurnState::Ended)
    })
    .await;
    assert!(next.input.contains("退出码为 3") && next.input.contains("two tests failed"), "{}", next.input);
    assert_eq!(next.task_id.as_deref(), Some(task.as_str()));
    assert_eq!(next.native_id, first.native_id, "the same session goes on");
    let v = db.read(|c| latest_verification(c, &task)).unwrap().unwrap();
    assert_eq!(v.state, VerificationState::Failed);
    let runs: Vec<(i64, Option<i64>)> = db
        .read(|c| {
            let mut stmt = c.prepare("SELECT seq, exit_code FROM check_run WHERE verification_id = ?1 ORDER BY seq")?;
            Ok(stmt.query_map([&v.id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    assert_eq!(runs, [(0, Some(0)), (1, Some(3))]);
    backend.shutdown(Duration::from_secs(5)).await;
}

/// A check that runs too long is ended with what it started, and fails (harness-adapter.md §4.2).
#[tokio::test]
async fn a_check_that_runs_too_long_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    set_config(&db, |config| {
        config.checks = vec![Check { command: r#"node -e "setTimeout(() => {}, 600000)""#.into(), timeout_secs: 2 }];
    });
    let task = create_task(&db, "FAKE:done");
    let first = ended_turn(&db).await;
    let next = wait_for("the turn after the timeout", || {
        last(&db).filter(|t| t.id != first.id && t.state == TurnState::Ended)
    })
    .await;
    assert!(next.input.contains("超时"), "{}", next.input);
    assert_eq!(db.read(|c| latest_verification(c, &task)).unwrap().unwrap().state, VerificationState::Failed);
    backend.shutdown(Duration::from_secs(5)).await;
}

/// A done whose capture is over the guardrail stops the role: nothing goes back to the executor
/// on its own. The user continues, and the executor gets the list (harness-adapter.md §4.1).
#[tokio::test]
async fn an_oversized_done_stops_the_role_until_the_user_decides() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    set_config(&db, |config| config.max_new_files = 0);
    let task = create_task(&db, "FAKE:done");
    let first = ended_turn(&db).await;
    wait_for("the capture to stop", || db.read(|c| stopped_capture(c, "Malkuth")).unwrap()).await;
    settle(&backend).await;
    assert_eq!(last(&db).unwrap().id, first.id, "no turn until the user decides");
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().phase, Phase::Executing);

    db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: "Malkuth".into() }).unwrap();
    backend.project.wake.notify_one();
    let next =
        wait_for("the continued turn", || last(&db).filter(|t| t.id != first.id && t.state == TurnState::Ended)).await;
    assert!(next.input.contains("work.txt") && next.input.contains("超过了项目设定的上限"), "{}", next.input);
    backend.shutdown(Duration::from_secs(5)).await;
}

/// Store work that fails stops with its reason and runs again only when the user retries
/// (data-model.md §5).
#[tokio::test]
async fn a_failed_store_job_waits_for_the_users_retry() {
    let dir = tempfile::tempdir().unwrap();
    // A file where the slot directory should be: writing the slot fails.
    let slot_path = slot(dir.path());
    std::fs::create_dir_all(slot_path.parent().unwrap()).unwrap();
    std::fs::write(&slot_path, "in the way").unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    create_task(&db, "FAKE:done");
    let failed = wait_for("the failure", || failures(&backend.project).into_iter().next()).await;
    assert_eq!(failed.0, "slot:worker");

    // The cause goes away; nothing happens until the user retries.
    std::fs::remove_file(&slot_path).unwrap();
    settle(&backend).await;
    assert!(last(&db).is_none());
    assert_eq!(retry_failed(&backend.project), 1);
    let turn = ended_turn(&db).await;
    assert_eq!(turn.outcome, Some(Outcome::Completed));
    backend.shutdown(Duration::from_secs(5)).await;
}

/// The preview never writes over the user's changes: it stops, and the user retries after
/// restoring the repository (harness-adapter.md §4.3).
#[tokio::test]
async fn the_preview_stops_on_local_changes_until_the_user_retries() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    let repo = user_repo(dir.path());
    let base = git(&repo, &["rev-parse", "HEAD"]);
    let task = create_task(&db, "FAKE:done");
    phase(&db, &task, Phase::Accepting).await;
    std::fs::write(repo.join("notes.txt"), "mine").unwrap();
    let v = db.read(|c| latest_verification(c, &task)).unwrap().unwrap();
    let accept = Accept {
        request_id: "a1".into(),
        task_id: task.clone(),
        verification_id: v.id,
        criteria_version: 1,
        expected_integration: base.clone(),
        dropping: vec![],
    };
    let published = db.execute(&Caller::User, &accept).unwrap();
    backend.project.wake.notify_one();
    let reason = wait_for("the preview to stop", || db.read(preview_stopped).unwrap()).await;
    assert!(reason.contains("notes.txt"), "{reason}");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), base, "nothing was written");
    assert_eq!(std::fs::read_to_string(repo.join("notes.txt")).unwrap(), "mine");

    std::fs::remove_file(repo.join("notes.txt")).unwrap();
    db.execute(&Caller::User, &RetryPreview { request_id: "r1".into() }).unwrap();
    backend.project.wake.notify_one();
    wait_for("the preview", || (git(&repo, &["rev-parse", "HEAD"]) == published).then_some(())).await;
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

    // The GUI offers continuing once the role holds, after the capture of the failed turn. A busy
    // machine may take longer than the wait above to capture it.
    wait_for("the role to hold", || db.read(|c| hold(c, "Malkuth")).unwrap()).await;
    db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: "Malkuth".into() }).unwrap();
    backend.project.wake.notify_one();
    let next =
        wait_for("the continued turn", || last(&db).filter(|t| t.id != failed.id && t.state == TurnState::Ended)).await;
    assert_eq!(next.outcome, Some(Outcome::Completed));
    assert_eq!(next.native_id, failed.native_id, "continue resumes the same native session");
    assert!(next.input.contains("失败") && next.input.contains("换个做法"), "{}", next.input);
    backend.shutdown(Duration::from_secs(5)).await;
}

/// A CLI that exits on its own with only stderr to show failed, and the failure says why. Only
/// stopping it makes a turn interrupted (data-model.md §3.2, #13).
#[tokio::test]
async fn a_cli_that_exits_with_only_stderr_fails_with_its_error() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start_with(dir.path(), fake_codex_with(&["--fake-start-error"])).await;
    let db = backend.project.db.clone();
    create_task(&db, "FAKE:done");

    let turn = ended_turn(&db).await;
    assert_eq!(turn.outcome, Some(Outcome::Failed));
    let message = turn.failure.unwrap().message;
    assert!(message.contains("退出码 1") && message.contains("Error: mcp_servers.lobotomy.url"), "{message}");
    assert!(message.contains("<token>") && !message.contains(&turn.token), "the token is left out: {message}");
    assert!(message.contains("  7: <unknown>"), "the excerpt keeps the start of stderr: {message}");
    let stderr = backend.project.raw_output_path(&turn.id, "stderr");
    assert!(stderr.exists(), "the raw stderr stays as evidence");
    backend.shutdown(Duration::from_secs(5)).await;
}

/// One backend per data directory: a second one cannot open it while the first lives (#16).
#[tokio::test]
async fn a_second_backend_cannot_open_the_same_data_directory() {
    let dir = tempfile::tempdir().unwrap();
    let first = open_project(dir.path(), fake_codex()).await;
    let host = Arc::new(Host::open(&host_dir(dir.path()), fake_codex()).unwrap());
    let data = dir.path().join("project");
    let refused = Project::open(&data, host.clone()).err().expect("the second open fails");
    assert!(refused.to_string().contains("another backend is already running"), "{refused:#}");
    drop(first);
    Project::open(&data, host).unwrap();
}

/// A turn whose session id cannot be recorded fails, and its raw output stays: otherwise the next
/// turn would quietly start a fresh session and lose the role's context (#16).
#[tokio::test]
async fn a_session_id_that_cannot_be_recorded_fails_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    let task = create_task(&db, "先说一句");
    let first = ended_turn(&db).await;
    assert_eq!(first.outcome, Some(Outcome::Completed));
    let message = SendMessage {
        request_id: "m1".into(),
        role: "Malkuth".into(),
        task_id: Some(task.clone()),
        body: "FAKE:newthread".into(),
    };
    db.execute(&Caller::User, &message).unwrap();
    let second =
        wait_for("the second turn to end", || last(&db).filter(|t| t.id != first.id && t.state == TurnState::Ended))
            .await;
    assert_eq!(second.outcome, Some(Outcome::Failed));
    let failure = second.failure.unwrap().message;
    assert!(failure.contains("没能记下这一轮的 Codex 会话 ID"), "{failure}");
    assert!(backend.project.raw_output_path(&second.id, "jsonl").exists(), "the raw output stays");
    wait_for("the role to hold", || db.read(|c| hold(c, "Malkuth")).unwrap()).await;
    backend.shutdown(Duration::from_secs(5)).await;
}

/// A Codex whose administrator does not allow full access refuses to start. The failure says the
/// permission mode was refused; after the user switches Codex to auto review, continuing sends
/// the brief that never arrived, and the task goes on (harness-adapter.md §1.9, data-model.md
/// §3.4).
#[tokio::test]
async fn a_managed_codex_runs_the_task_after_switching_to_auto_review() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    let task = create_task(&db, "FAKE:managed FAKE:done");

    let refused = ended_turn(&db).await;
    let failure = refused.failure.clone().unwrap();
    assert_eq!(refused.outcome, Some(Outcome::Failed));
    assert_eq!((failure.kind, failure.unstarted), (FailureKind::Permission, true), "{failure:?}");
    let bound = |turn: &str| -> Vec<(String, Option<i64>)> {
        db.read(|c| {
            let mut stmt = c.prepare("SELECT id, delivered_at FROM message WHERE turn_id = ?1 ORDER BY seq")?;
            Ok(stmt.query_map([turn], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap()
    };
    let brief = bound(&refused.id);
    assert_eq!(brief.len(), 1, "the brief was bound to the refused turn");

    backend.project.host.set_permission("codex", "auto_review").unwrap();
    // The GUI offers continuing once the role holds, after the capture of the failed turn.
    wait_for("the role to hold", || db.read(|c| hold(c, "Malkuth")).unwrap()).await;
    db.execute(&Caller::User, &Continue { request_id: "c1".into(), role: "Malkuth".into() }).unwrap();
    let next = wait_for("the continued turn to end", || {
        last(&db).filter(|t| t.id != refused.id && t.state == TurnState::Ended)
    })
    .await;
    assert_eq!(next.outcome, Some(Outcome::Completed), "{:?}", next.failure);
    assert!(next.done_at.is_some(), "org_report(done) went through");
    assert_eq!(next.input, refused.input, "the input goes again as it was, with no note about the refusal");
    let moved = bound(&next.id);
    assert_eq!(moved.iter().map(|m| &m.0).collect::<Vec<_>>(), [&brief[0].0], "the brief moved to the new turn");
    assert!(moved[0].1.is_some(), "and was delivered there");
    assert!(bound(&refused.id).is_empty());

    let args = std::fs::read_to_string(diag(&slot(dir.path())).join("last-args.json")).unwrap();
    assert!(args.contains(r#"approvals_reviewer=\"auto_review\""#), "{args}");
    assert!(!args.contains("--dangerously-bypass-approvals-and-sandbox"), "{args}");
    phase(&db, &task, Phase::Accepting).await;
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
        let project = open_project(dir.path(), fake_codex()).await;
        create_task(&project.db, "FAKE:done");
        start_attempt(&project, &next_task(&project.db)).await;
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
    // Nothing is missing, so the raw output goes once the runner is done.
    let raw = backend.project.raw_output_path(&turn.id, "jsonl");
    wait_for("the raw output to go", || (!raw.exists()).then_some(())).await;
    backend.shutdown(Duration::from_secs(5)).await;
}

/// One real Codex turn on the user's subscription: the role writes a file and reports done.
#[tokio::test]
#[ignore = "runs a real Codex turn"]
async fn a_real_codex_turn_reports_done() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start_with(dir.path(), real_codex()).await;
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
        let deleted = std::process::Command::new(&backend.project.host.harness.codex.command[0])
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
    // Codex leaves nothing else in the slot that a capture would pick up.
    assert_eq!(git(&slot(dir.path()), &["status", "--porcelain", "--untracked-files=all"]), "?? hello.txt");
    let task = turn.task_id.clone().unwrap();
    phase(&db, &task, Phase::Accepting).await;
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
        let project = open_project(dir.path(), fake_codex()).await;
        let task = create_task(&project.db, "FAKE:done");
        start_attempt(&project, &task).await;
        let turn = project.db.execute(&Caller::Runtime, &RegisterTurn { role: "Malkuth".into() }).unwrap();
        let failure = Failure::new(FailureKind::Quota, "limit");
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

    wait_for("the role to hold", || db.read(|c| hold(c, "Malkuth")).unwrap()).await;
    db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: "Malkuth".into() }).unwrap();
    backend.project.wake.notify_one();
    let next =
        wait_for("the continued turn", || last(&db).filter(|t| t.id != failed.id && t.state == TurnState::Ended)).await;
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
