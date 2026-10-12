//! The backend end to end with a fake Codex and a fake Claude (tests/fixtures/fake-*.mjs, run by
//! Node): the scheduler, the turn runner, the MCP service and the database together. Needs `node`
//! on PATH.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use lobotomy_core::capture::stopped_capture;
use lobotomy_core::id::now_ms;
use lobotomy_core::project::{Check, EditProjectConfig, ProjectConfig, current_config, load_project};
use lobotomy_core::task::{Abandon, CreateTask, Phase, SendMessage, SetPaused, load_task, open_attempt};
use lobotomy_core::turn::{
    Continue, EndTurn, Failure, FailureKind, Outcome, RegisterTurn, SetRoleHarness, TurnState, hold, load_turn,
};
use lobotomy_core::verify::{Accept, RetryPreview, VerificationState, latest_verification, preview_stopped};
use lobotomy_core::{Caller, Db};
use lobotomyd::host::Host;
use lobotomyd::instance::Instance;
use lobotomyd::project::Project;
use lobotomyd::results::{failures, retry_failed};
use serde_json::{Value, json};

/// The M1 chain: the user creates a task, Malkuth works in its slot and reports done, the runtime
/// captures and verifies the candidate, the user accepts it, and the user's repository is
/// fast-forwarded to the new integration version.
#[tokio::test]
async fn a_task_runs_from_done_to_the_users_repository() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    // Leave this direct database write unnotified: work must eventually progress even when
    // the command did not arrive through the GUI's wake path.
    let task = db
        .execute(
            &Caller::User,
            &CreateTask {
                request_id: "c-FAKE:done".into(),
                title: "测试任务".into(),
                body: "FAKE:done".into(),
                criteria: "work.txt 存在".into(),
                executor: "Malkuth".into(),
            },
        )
        .unwrap()
        .id;

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
        &SendMessage {
            request_id: "m1".into(),
            role: "Malkuth".into(),
            task_id: None,
            body: "还有一件事".into(),
            reply_to: None,
        },
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

/// Checks run on the candidate and a failure goes back to the executor (harness-adapter.md
/// §4.2). Observe after both turns finish: the done turn must still be identified, rather than
/// treating the latest turn as the first (#25).
#[tokio::test]
async fn a_failed_check_can_finish_before_the_first_turn_is_observed() {
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
    let task = create_task(&backend.project, "FAKE:done");
    wait_for_backend(&backend.project, "both turns to end before observing the first", || {
        let ended: i64 = db
            .read(|c| {
                Ok(c.query_row("SELECT count(*) FROM turn WHERE task_id = ?1 AND state = 'ended'", [&task], |r| {
                    r.get(0)
                })?)
            })
            .unwrap();
        (ended == 2).then_some(())
    })
    .await;
    // The attempt records the exact turn that reported done. This remains true when the
    // resumed turn has already started or ended before the test's first poll.
    let first = wait_for_backend(&backend.project, "the reported done turn to end", || {
        db.read(|c| {
            use rusqlite::OptionalExtension as _;
            let id: Option<String> = c
                .query_row("SELECT done_turn_id FROM attempt WHERE task_id = ?1 ORDER BY seq LIMIT 1", [&task], |r| {
                    r.get(0)
                })
                .optional()?
                .flatten();
            id.map(|id| load_turn(c, &id)).transpose()
        })
        .unwrap()
        .filter(|t| t.state == TurnState::Ended)
    })
    .await;
    assert!(first.done_at.is_some(), "the first turn reported done");
    assert_eq!(first.outcome, Some(Outcome::Completed));
    assert_ne!(first.id, last(&db).unwrap().id, "the latest turn is already the resumed turn");

    // The first check saw the candidate's file; the second failed, and its output went back.
    let next = wait_for_backend(&backend.project, "the turn after the failed check", || {
        last(&db).filter(|t| t.id != first.id && t.state == TurnState::Ended)
    })
    .await;
    assert_eq!(next.outcome, Some(Outcome::Completed));
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
    let task = create_task(&backend.project, "FAKE:done");
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
    let task = create_task(&backend.project, "FAKE:done");
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
    create_task(&backend.project, "FAKE:done");
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
    let task = create_task(&backend.project, "FAKE:done");
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
    create_task(&backend.project, "FAKE:fail");

    let failed = ended_turn(&db).await;
    assert_eq!(failed.outcome, Some(Outcome::Failed));
    assert_eq!(failed.failure.as_ref().map(|f| f.message.as_str()), Some("boom"));
    assert!(backend.project.raw_output_path(&failed.id, "jsonl").exists(), "an abnormal turn keeps its raw output");

    db.execute(
        &Caller::User,
        &SendMessage {
            request_id: "m1".into(),
            role: "Malkuth".into(),
            task_id: None,
            body: "换个做法".into(),
            reply_to: None,
        },
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
    create_task(&backend.project, "FAKE:done");

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
    let id = first.id.clone();
    let host = Arc::new(Host::open(&host_dir(dir.path()), fake_codex()).unwrap());
    let data = dir.path().join("project");
    let refused = Project::open(&id, &data, host.clone()).err().expect("the second open fails");
    assert!(refused.to_string().contains("another backend is already running"), "{refused:#}");
    drop(first);
    Project::open(&id, &data, host).unwrap();
}

/// Before multi-project, `gui.json` in the host directory named the one project. The first start
/// after the upgrade registers it where it is and opens it (data-model.md §10.7).
#[tokio::test]
async fn a_project_from_before_multi_project_is_registered_and_opened() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::open(&host_dir(dir.path()), fake_codex()).unwrap());
    let data = dir.path().join("old-project");
    let id = {
        let project = Arc::new(Project::open("prj_old", &data, host.clone()).unwrap());
        lobotomyd::onboard::onboard(&project, &user_repo(dir.path())).await.unwrap();
        create_task(&project, "上次的任务");
        project.id.clone()
    };
    let config = serde_json::json!({ "project": data });
    std::fs::write(host.dir.join("gui.json"), config.to_string()).unwrap();

    let backend = lobotomyd::Backend::start(host.clone(), 0).await.unwrap();
    let project = backend.project(&id).expect("the old project is open");
    assert_eq!(project.data_dir, data, "its data directory stays where it was");
    let entry = host.db.project(&id).unwrap().unwrap();
    assert_eq!(
        (entry.name.as_str(), entry.repo_path.as_str()),
        ("repo", project.db.read(load_project).unwrap().unwrap().repo_path.as_str())
    );
    let tasks: i64 = project.db.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM task", [], |r| r.get(0))?)).unwrap();
    assert_eq!(tasks, 1, "its data is kept");
    drop(project);
    backend.shutdown(Duration::from_secs(5)).await;
}

/// An idle backend with no GUI clients, turns or store jobs releases its project on shutdown,
/// so the data directory can be reopened immediately in the same runtime without lock retries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_backend_releases_its_project_lock_on_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let (host, id) = idle_project(dir.path(), fake_codex()).await;
    let data = dir.path().join("project");
    for _ in 0..20 {
        let backend = lobotomyd::Backend::start(host.clone(), 0).await.unwrap();
        assert!(backend.project(&id).is_some(), "the project opened");
        backend.shutdown(Duration::from_secs(5)).await;
        // No sleep or lock retry: shutdown returning is the synchronization point.
        drop(Project::open(&id, &data, host.clone()).unwrap());
    }
}

/// A turn whose session id cannot be recorded fails, and its raw output stays: otherwise the next
/// turn would quietly start a fresh session and lose the role's context (#16).
#[tokio::test]
async fn a_session_id_that_cannot_be_recorded_fails_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    let task = create_task(&backend.project, "先说一句");
    let first = ended_turn(&db).await;
    assert_eq!(first.outcome, Some(Outcome::Completed));
    let message = SendMessage {
        request_id: "m1".into(),
        role: "Malkuth".into(),
        task_id: Some(task.clone()),
        body: "FAKE:newthread".into(),
        reply_to: None,
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
    let task = create_task(&backend.project, "FAKE:managed FAKE:done");

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
    create_task(&backend.project, "FAKE:sleep");

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
        create_task(&project, "FAKE:done");
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
    create_task(&backend.project, "FAKE:big");

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
    let backend = start_with(dir.path(), real_clis()).await;
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

/// One real Claude turn on the user's subscription, on the smallest model: the role writes a file
/// and reports done through the turn's MCP configuration (M2).
#[tokio::test]
#[ignore = "runs a real Claude Code turn"]
async fn a_real_claude_turn_reports_done() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start_with(dir.path(), real_clis()).await;
    let db = backend.project.db.clone();
    on_claude(&db);
    db.write(|tx| Ok(tx.execute("UPDATE role SET model = 'haiku' WHERE name = 'Malkuth'", [])?)).unwrap();
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
    // A clean turn leaves no raw output. What is kept had lines the parser does not know.
    tokio::time::sleep(Duration::from_millis(500)).await;
    if let Ok(raw) = std::fs::read_to_string(backend.project.raw_output_path(&turn.id, "jsonl")) {
        let mut parser = lobotomy_harness::claude::Parser::default();
        for line in raw.lines() {
            for event in parser.parse_line(line) {
                if matches!(
                    event,
                    lobotomy_harness::event::Event::Unknown(_) | lobotomy_harness::event::Event::Unparsed(_)
                ) {
                    eprintln!("not known: {}", &line[..line.len().min(300)]);
                }
            }
        }
    }
    // Role sessions persist so they can resume; this one is a test, so keep it out of the user's
    // Claude history: its file is ~/.claude/projects/<the slot's path>/<session>.jsonl.
    if let (Some(session), Some(home)) = (&turn.native_id, std::env::var_os("USERPROFILE").or(std::env::var_os("HOME")))
    {
        let projects = std::path::Path::new(&home).join(".claude").join("projects");
        for project in std::fs::read_dir(&projects).into_iter().flatten().flatten() {
            let file = project.path().join(format!("{session}.jsonl"));
            if file.exists() {
                eprintln!("deleted Claude session {session}: {:?}", std::fs::remove_file(&file));
            }
        }
    }
    assert_eq!((turn.harness.as_str(), turn.outcome), ("claude", Some(Outcome::Completed)));
    assert!(turn.done_at.is_some(), "the role reported done");
    assert!(items.iter().any(|(kind, id)| kind == "mcp_call" && id.is_some()), "the call refers to its command record");
    let hello = std::fs::read_to_string(slot(dir.path()).join("hello.txt")).unwrap();
    assert_eq!(hello.trim(), "hi");
    // Claude leaves nothing else in the slot that a capture would pick up.
    assert_eq!(git(&slot(dir.path()), &["status", "--porcelain", "--untracked-files=all"]), "?? hello.txt");
    let task = turn.task_id.clone().unwrap();
    phase(&db, &task, Phase::Accepting).await;
    backend.shutdown(Duration::from_secs(5)).await;
}

/// A real Claude turn interrupted while its command runs ends as interrupted, and continuing resumes
/// the same session (harness-adapter.md §1.3 rule 6, §1.7): the native interrupt on Claude (M2).
#[tokio::test]
#[ignore = "runs real Claude Code turns"]
async fn a_real_claude_turn_is_interrupted_and_continued() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start_with(dir.path(), real_clis()).await;
    let db = backend.project.db.clone();
    on_claude(&db);
    db.write(|tx| Ok(tx.execute("UPDATE role SET model = 'haiku' WHERE name = 'Malkuth'", [])?)).unwrap();
    let body = "运行这条命令并等它结束：Start-Sleep -Seconds 120。然后报告完成。";
    create_task(&backend.project, body);

    let turn = wait_for("the turn to run", || last(&db).filter(|t| t.state == TurnState::Running)).await;
    let project = backend.project.clone();
    wait_for("the command to run", || {
        let live = project.live.lock().unwrap();
        live.get(&turn.id).is_some_and(|t| t.items.values().any(|i| i.kind == "command")).then_some(())
    })
    .await;
    let asked = Instant::now();
    lobotomyd::runner::interrupt(&backend.project, &turn.id).await.unwrap();
    let ended = ended_turn(&db).await;
    eprintln!("ended {:?} after {:?}", ended.outcome, asked.elapsed());
    let raw = std::fs::read_to_string(backend.project.raw_output_path(&ended.id, "jsonl")).unwrap_or_default();
    for line in raw.lines().filter(|l| l.contains("\"type\":\"result\"")) {
        eprintln!("result on Ctrl+C: {}", &line[..line.len().min(400)]);
    }
    assert_eq!(ended.outcome, Some(Outcome::Interrupted));
    assert!(asked.elapsed() < Duration::from_secs(60), "Ctrl+C stopped the turn, not the command's end");
    assert!(!lobotomy_harness::process::is_running(ended.pid.unwrap() as u32, ended.process_start.unwrap()));

    wait_for("the role to hold", || db.read(|c| hold(c, "Malkuth")).unwrap()).await;
    let reply = SendMessage {
        request_id: "m1".into(),
        role: "Malkuth".into(),
        task_id: ended.task_id.clone(),
        body: "不用再等了，直接报告完成。".into(),
        reply_to: None,
    };
    db.execute(&Caller::User, &reply).unwrap();
    db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: "Malkuth".into() }).unwrap();
    backend.project.wake.notify_one();
    let next =
        wait_for("the continued turn", || last(&db).filter(|t| t.id != ended.id && t.state == TurnState::Ended)).await;
    eprintln!("continued {:?}, done {:?}", next.outcome, next.done_at);
    if let (Some(session), Some(home)) = (&next.native_id, std::env::var_os("USERPROFILE").or(std::env::var_os("HOME")))
    {
        let projects = std::path::Path::new(&home).join(".claude").join("projects");
        for project in std::fs::read_dir(&projects).into_iter().flatten().flatten() {
            let _ = std::fs::remove_file(project.path().join(format!("{session}.jsonl")));
        }
    }
    assert_eq!(next.native_id, ended.native_id, "continuing resumes the same session");
    assert_eq!(next.outcome, Some(Outcome::Completed));
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
    let task = create_task(&backend.project, "FAKE:done");
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
        let task = create_task(&project, "FAKE:done");
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
    let a = create_task(&backend.project, "FAKE:fail");
    let failed = ended_turn(&db).await;
    assert_eq!(failed.outcome, Some(Outcome::Failed));
    let b = create_task(&backend.project, "FAKE:done");
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
    let host = Arc::new(Host::open(dir.path(), real_clis()).unwrap());
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

/// Archiving stops one project as quitting does (data-model.md §10.4): a CLI that does not exit
/// when interrupted ends with its process tree once the grace period is over, and the data
/// directory is free. Unarchiving opens the project as at startup: the turn ends as interrupted
/// and its role waits for the user.
#[tokio::test]
async fn archiving_ends_a_cli_that_ignores_the_interrupt_and_unarchiving_reconciles_its_turn() {
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
    let mut harness = fake_codex();
    harness.codex.command = vec!["node".into(), script.to_string_lossy().into_owned()];
    let Started { backend, project, .. } = start_with(dir.path(), harness).await;
    let (id, data, host) = (project.id.clone(), project.data_dir.clone(), project.host.clone());
    let db = project.db.clone();
    create_task(&project, "deaf");
    let turn =
        wait_for("the turn to run", || last(&db).filter(|t| t.state == TurnState::Running && t.pid.is_some())).await;
    // The handlers and the ordinary child are ready before the interrupt; a launched pid alone
    // could let SIGINT end Node before it installed its handler and hide the forced-stop bug.
    let child: u32 =
        wait_for("the process tree to be ready", || std::fs::read_to_string(&ready).ok()?.parse().ok()).await;
    assert!(child_is_running(child));
    drop(project);

    backend.projects.archive_within(&id, Duration::from_secs(1)).await.unwrap();
    assert!(backend.projects.get(&id).is_none());
    let (pid, start) = (turn.pid.unwrap() as u32, turn.process_start.unwrap());
    assert!(!lobotomy_harness::process::is_running(pid, start), "archive returns after the CLI exits");
    assert!(!child_is_running(child), "archive returns after the ordinary child exits");
    drop(Project::open(&id, &data, host.clone()).expect("the archived project's directory is free"));
    let snapshot = host.db.project(&id).unwrap().unwrap();
    assert_eq!(snapshot.state, lobotomy_core::host::ProjectState::Archived);

    backend.projects.unarchive(&id).await.unwrap();
    let reopened = backend.projects.get(&id).expect("the project opened again");
    let ended = ended_turn(&reopened.db).await;
    assert_eq!((ended.id.as_str(), ended.outcome), (turn.id.as_str(), Some(Outcome::Interrupted)));
    // Once the runtime has captured the interrupted turn's scene.
    wait_for("the role to wait for the user to continue", || reopened.db.read(|c| hold(c, "Malkuth")).unwrap()).await;
    drop(reopened);
    backend.shutdown(Duration::from_secs(5)).await;
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

fn next_task(db: &Db) -> String {
    db.read(|c| lobotomy_core::task::next_queued_task(c, "Malkuth")).unwrap().unwrap().id
}

// ---- Malkuth on Claude (M2), with the fake Claude (tests/fixtures/fake-claude.mjs) ----

/// Puts Malkuth on Claude before any work, as the user does in the settings.
fn on_claude(db: &Db) {
    let switch = SetRoleHarness { request_id: "h1".into(), role: "Malkuth".into(), harness: "claude".into() };
    db.execute(&Caller::User, &switch).unwrap();
}

/// The arguments of the fake CLI's last run in `dir`.
fn last_args(dir: &std::path::Path) -> Vec<String> {
    serde_json::from_str(&std::fs::read_to_string(diag(dir).join("last-args.json")).unwrap()).unwrap()
}

fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
    &args[args.iter().position(|a| a == flag).unwrap_or_else(|| panic!("no {flag} in {args:?}")) + 1]
}

fn item_kinds(db: &Db, turn: &str) -> Vec<String> {
    db.read(|c| {
        let mut stmt = c.prepare("SELECT kind FROM item WHERE turn_id = ?1 ORDER BY seq")?;
        Ok(stmt.query_map([turn], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    })
    .unwrap()
}

/// Claude ends an interrupted turn with an error result. The user asked for the interruption, so
/// the turn is interrupted, not failed: continuing it is the same choice as after Codex.
#[tokio::test]
async fn interrupting_a_claude_turn_ends_it_as_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    on_claude(&db);
    create_task(&backend.project, "FAKE:sleep");

    let turn = wait_for("the turn to run", || last(&db).filter(|t| t.state == TurnState::Running)).await;
    // Give the fake time to install its SIGINT handler.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    lobotomyd::runner::interrupt(&backend.project, &turn.id).await.unwrap();
    let ended = ended_turn(&db).await;
    assert_eq!((ended.outcome, ended.failure), (Some(Outcome::Interrupted), None));
    backend.shutdown(Duration::from_secs(5)).await;
}

/// The M1 chain on Claude: the runtime names the session, Claude reports done through the MCP
/// configuration file of the turn, and the candidate goes to acceptance.
#[tokio::test]
async fn a_task_runs_on_claude_to_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    on_claude(&db);
    let task = create_task(&backend.project, "FAKE:done");

    let turn = ended_turn(&db).await;
    assert_eq!((turn.harness.as_str(), turn.outcome), ("claude", Some(Outcome::Completed)));
    assert!(turn.done_at.is_some(), "org_report(done) reached the turn");
    let args = last_args(&slot(dir.path()));
    assert_eq!(turn.native_id.as_deref(), Some(after(&args, "--session-id")), "Claude kept the session it was given");
    assert!(after(&args, "--append-system-prompt").contains("你是 Malkuth"));
    // The configuration held the turn's token; it goes with the turn.
    let config = std::path::PathBuf::from(after(&args, "--mcp-config"));
    assert!(config.file_name().unwrap().to_string_lossy().starts_with(&turn.id));
    wait_for("the MCP configuration to go", || (!config.exists()).then_some(())).await;

    assert_eq!(
        item_kinds(&db, &turn.id),
        ["input", "agent_message", "file_change", "tool_call", "mcp_call", "agent_message"]
    );
    phase(&db, &task, Phase::Accepting).await;
    backend.shutdown(Duration::from_secs(5)).await;
}

/// The next turn resumes the session the runtime named for the first.
#[tokio::test]
async fn a_claude_turn_resumes_its_session() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let db = backend.project.db.clone();
    on_claude(&db);
    let task = create_task(&backend.project, "FAKE:blocked");
    let first = ended_turn(&db).await;
    let reply = SendMessage {
        request_id: "m1".into(),
        role: "Malkuth".into(),
        task_id: Some(task),
        body: "FAKE:done".into(),
        reply_to: None,
    };
    db.execute(&Caller::User, &reply).unwrap();
    backend.project.wake.notify_one();

    let next =
        wait_for("the next turn", || last(&db).filter(|t| t.id != first.id && t.state == TurnState::Ended)).await;
    assert_eq!(next.native_id, first.native_id);
    assert_eq!(Some(after(&last_args(&slot(dir.path())), "--resume")), first.native_id.as_deref());
    backend.shutdown(Duration::from_secs(5)).await;
}

/// The quota domain belongs to the host (data-model.md §10.5, roadmap M3 acceptance): Claude's
/// session limit in one project stops Claude's roles in every project, Codex's roles go on, and
/// the user's retry lets them all continue.
#[tokio::test]
async fn a_session_limit_in_one_project_stops_claude_in_every_project() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let a = backend.project.clone();
    on_claude(&a.db);
    create_task(&a, "FAKE:quota");
    let turn = ended_turn(&a.db).await;
    let failure = turn.failure.clone().unwrap();
    assert_eq!(
        (turn.outcome, failure.kind, failure.resets_at),
        (Some(Outcome::Failed), FailureKind::Quota, Some(1_790_868_000_000))
    );
    assert!(failure.message.contains("session limit"), "{failure:?}");
    assert!(a.host.is_blocked("claude").unwrap());
    assert!(!a.host.is_blocked("codex").unwrap());

    let projects = backend.backend.projects.clone();
    let open = |name: &str| {
        let projects = projects.clone();
        let repo = user_repo(&dir.path().join(name));
        async move {
            let entry = projects.create(&repo).await.unwrap();
            projects.get(&entry.id).unwrap()
        }
    };
    let (b, c) = (open("b").await, open("c").await);
    on_claude(&b.db);
    create_task(&b, "FAKE:done");
    create_task(&c, "FAKE:done");
    assert_eq!(ended_turn(&c.db).await.outcome, Some(Outcome::Completed), "Codex goes on");
    settle(&backend).await;
    assert!(last(&b.db).is_none(), "Claude starts nothing in another project");

    assert!(!a.host.retry("claude").await.unwrap().is_blocked());
    assert!(last_args(&host_dir(dir.path()).join("probe")).contains(&"--no-session-persistence".to_owned()));
    projects.wake_all();
    assert_eq!(ended_turn(&b.db).await.outcome, Some(Outcome::Completed));
    drop((a, b, c));
    backend.shutdown(Duration::from_secs(5)).await;
}

/// Shutdown does not wait for store jobs: an unfinished one runs again after a restart. It must
/// not abort the job or release the project lock while the job's side effects are still running.
#[tokio::test]
async fn shutdown_leaves_a_running_store_job_and_its_lock_alive() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let (host, id, data) = (backend.project.host.clone(), backend.project.id.clone(), dir.path().join("project"));
    let project = Arc::downgrade(&backend.project);
    let (release, gate) = tokio::sync::oneshot::channel();
    let (finished, done) = tokio::sync::oneshot::channel();
    lobotomyd::results::spawn_job(&backend.project, "test:gate".into(), async move {
        gate.await.unwrap();
        finished.send(()).unwrap();
        Ok(())
    });
    // A grace longer than the timeout: returning in time shows shutdown did not wait for the job.
    tokio::time::timeout(Duration::from_secs(5), backend.shutdown(Duration::from_secs(30))).await.unwrap();
    assert!(Project::open(&id, &data, host.clone()).is_err());
    let project = project.upgrade().expect("unfinished store work owns the project");
    release.send(()).unwrap();
    done.await.unwrap();
    wait_for("the store job to release its project", || (Arc::strong_count(&project) == 1).then_some(())).await;
    // Drop the last owner here: observing a zero weak count alone can race its destructor.
    drop(project);
    drop(Project::open(&id, &data, host).unwrap());
}

/// A trial runs the executor's command in a copy of the candidate, in a terminal of its own. Stop
/// ends it and deletes the copy; shutdown stops a trial still open; startup deletes leftovers
/// (frontend.md §8). Linux needs a desktop, which CI does not have.
#[tokio::test]
async fn a_trial_runs_in_a_copy_and_ends_with_stop_or_shutdown() {
    use lobotomyd::trial::{self, StartTrial};
    if cfg!(unix) && std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let project = backend.project.clone();
    let db = project.db.clone();
    let task = create_task(&backend.project, "FAKE:done FAKE:trial");
    phase(&db, &task, Phase::Accepting).await;
    let request = || {
        let detail = db.read(|c| lobotomy_core::view::task_detail(c, &task)).unwrap();
        let attempt = detail.attempts.last().unwrap();
        let candidate = detail.captures.iter().find(|c| Some(&c.id) == attempt.candidate_id.as_ref()).unwrap();
        StartTrial {
            task_id: task.clone(),
            attempt_id: attempt.id.clone(),
            expected_candidate: candidate.commit_id.clone().unwrap(),
        }
    };
    let copy = |view: &trial::TrialView| std::path::PathBuf::from(&view.directory);
    let gone = |view: &trial::TrialView| {
        let root = copy(view).parent().unwrap().to_path_buf();
        move || (!root.exists()).then_some(())
    };

    let first = trial::start(&project, request()).await.unwrap();
    let started = copy(&first).join("started.txt");
    wait_for("the trial command to start in the copy", || started.exists().then_some(())).await;
    assert_eq!(std::fs::read_to_string(copy(&first).join("work.txt")).unwrap(), "hi");
    assert_eq!(trial::start(&project, request()).await.unwrap().id, first.id, "one open trial per round");
    project.trials.stop(&first.id).await.unwrap();
    assert_eq!(project.trials.list(&task)[0].state, "closed");
    wait_for("the stopped trial's copy to go", gone(&first)).await;

    let second = trial::start(&project, request()).await.unwrap();
    assert_ne!(second.directory, first.directory, "a closed trial starts again in a new copy");
    let started = copy(&second).join("started.txt");
    wait_for("the second trial to start", || started.exists().then_some(())).await;
    backend.shutdown(Duration::from_secs(5)).await;
    assert!(project.trials.list(&task).iter().all(|t| t.state == "closed"), "shutdown stops open trials");
    wait_for("the second copy to go", gone(&second)).await;

    let leftover = project.trials_dir().join("trial_left").join("candidate");
    std::fs::create_dir_all(&leftover).unwrap();
    lobotomyd::results::sweep(&project).await.unwrap();
    assert!(!project.trials_dir().join("trial_left").exists(), "startup deletes leftover copies");
}

// ---- Angela (roadmap.md M4 step 1), on the fake Claude ----

/// Wiring only: both harnesses receive the complete manager prompt on initial and resumed
/// turns. The fake CLIs do not evaluate whether a model follows the behavioral examples.
#[tokio::test]
async fn angela_receives_the_manager_prompt_on_both_harnesses_each_turn() {
    let expected = include_str!("../src/prompts/manager.md").replace("{name}", "Angela");
    for harness in ["claude", "codex"] {
        let dir = tempfile::tempdir().unwrap();
        let backend = start(dir.path()).await;
        let angela = backend.backend.angela().unwrap();
        angela
            .db
            .execute(
                &Caller::User,
                &SetRoleHarness { request_id: "h1".into(), role: "Angela".into(), harness: harness.into() },
            )
            .unwrap();
        let mut native_id = None;
        for message in ["先聊聊", "继续讨论"] {
            let asked = tell_angela(&angela, message);
            let turn = angela_turn_for(&angela, &asked).await;
            assert_eq!(turn.outcome, Some(Outcome::Completed));
            let args = last_args(&host_dir(dir.path()).join("angela").join("work"));
            let received = if harness == "claude" {
                after(&args, "--append-system-prompt").to_owned()
            } else {
                let value = args.iter().find_map(|arg| arg.strip_prefix("developer_instructions=")).unwrap();
                serde_json::from_str::<String>(value).unwrap()
            };
            assert_eq!(received, expected, "{harness} must receive the runtime prompt unchanged");
            if let Some(id) = &native_id {
                assert_eq!(after(&args, if harness == "claude" { "--resume" } else { "resume" }), id);
                assert_eq!(turn.native_id.as_ref(), Some(id));
            }
            native_id = Some(turn.native_id.expect("the CLI identified the session"));
        }
        backend.shutdown(Duration::from_secs(5)).await;
    }
}

/// The user's message to Angela, as the window sends it; its id.
fn tell_angela(angela: &Instance, body: &str) -> String {
    let message = SendMessage {
        request_id: format!("a-{body}"),
        role: "Angela".into(),
        task_id: None,
        body: body.into(),
        reply_to: None,
    };
    let id = angela.db.execute(&Caller::User, &message).unwrap().id;
    angela.wake.notify_one();
    id
}

/// Angela's turn that delivered the message `id`, once it ended. A project that settles can wake
/// her in between (manager-actions.md §6), so her last turn may be another one.
async fn angela_turn_for(angela: &Instance, id: &str) -> lobotomy_core::turn::Turn {
    wait_for("Angela's turn to end", || {
        let turn: Option<String> = angela
            .db
            .read(|c| Ok(c.query_row("SELECT turn_id FROM message WHERE id = ?1", [id], |r| r.get(0))?))
            .unwrap();
        let turn = turn?;
        let turn = angela.db.read(|c| load_turn(c, &turn)).unwrap();
        (turn.state == TurnState::Ended).then_some(turn)
    })
    .await
}

/// Angela's last turn, once it ended.
async fn angela_turn_ended(angela: &Instance) -> lobotomy_core::turn::Turn {
    wait_for("Angela's turn to end", || {
        angela.db.read(|c| lobotomy_core::turn::last_turn(c, "Angela")).unwrap().filter(|t| t.state == TurnState::Ended)
    })
    .await
}

/// The replies of the Lobotomy tools Angela called in a turn, in order, as JSON.
fn tool_replies(db: &Db, turn: &str) -> Vec<Value> {
    db.read(|c| {
        let mut stmt =
            c.prepare("SELECT content, command_id FROM item WHERE turn_id = ?1 AND kind = 'mcp_call' ORDER BY seq")?;
        let rows = stmt.query_map([turn], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .unwrap()
    .into_iter()
    .map(|(content, command)| {
        assert_eq!(command, None, "a read is no command of hers");
        let content: Value = serde_json::from_str(&content).unwrap();
        serde_json::from_str(content["result_text"].as_str().unwrap()).unwrap()
    })
    .collect()
}

/// Angela reads every project through her tools (data-model.md §10.6 "读取带边界"): each read
/// says up to which event it is, an archived project is named as unavailable with the reason
/// rather than left out, and a task's candidate and verification say whether they still hold.
#[tokio::test]
async fn angela_reads_every_project_with_the_bounds_of_each_read() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let angela = backend.backend.angela().expect("Angela opened");
    let project = backend.project.clone();
    let task = create_task(&project, "FAKE:done");
    phase(&project.db, &task, Phase::Accepting).await;
    let projects = backend.backend.projects.clone();
    let archived = projects.create(&user_repo(&dir.path().join("b"))).await.unwrap().id;
    projects.archive(&archived).await.unwrap();

    let calls = [
        "FAKE:call=list_projects".to_owned(),
        format!(r#"FAKE:call=read_project{{"project_id":"{}"}}"#, project.id),
        format!(r#"FAKE:call=read_task{{"project_id":"{}","task_id":"{task}"}}"#, project.id),
        format!(r#"FAKE:call=read_events{{"project_id":"{}","limit":3}}"#, project.id),
        format!(r#"FAKE:call=read_project{{"project_id":"{archived}"}}"#),
    ];
    let asked = tell_angela(&angela, &calls.join("\n"));
    let turn = angela_turn_for(&angela, &asked).await;
    assert_eq!((turn.outcome, turn.workspace_id.as_deref()), (Some(Outcome::Completed), None));
    let args = last_args(&host_dir(dir.path()).join("angela").join("work"));
    assert!(after(&args, "--append-system-prompt").contains("你是 Angela"), "she runs in her own directory");

    let replies = tool_replies(&angela.db, &turn.id);
    let [list, read, task_read, events, gone] = replies.as_slice() else { panic!("five replies: {replies:#?}") };
    let as_of = project.db.read(lobotomy_core::view::last_event).unwrap();
    let entries = list["projects"].as_array().unwrap();
    let open = entries.iter().find(|p| p["id"] == project.id.as_str()).unwrap();
    assert_eq!((open["as_of"].as_i64(), open["open_tasks"].as_i64()), (Some(as_of), Some(1)));
    let closed = entries.iter().find(|p| p["id"] == archived.as_str()).unwrap();
    assert_eq!((closed["unavailable"].as_str(), &closed["as_of"]), (Some("已归档"), &Value::Null));

    assert_eq!(read["read"]["as_of"].as_i64(), Some(as_of));
    assert_eq!(read["read"]["tasks"][0]["id"], task.as_str());
    assert_eq!(read["read"]["waiting"][0]["kind"], "accept");

    let task_read = &task_read["read"];
    assert_eq!(
        (task_read["body"].as_str(), task_read["criteria"].as_str()),
        (Some("FAKE:done"), Some("work.txt 存在"))
    );
    let candidate = &task_read["attempts"][0]["candidate"];
    assert_eq!(candidate["superseded"], false);
    assert_eq!(
        (&candidate["verifications"][0]["state"], &candidate["verifications"][0]["stale"]),
        (&"passed".into(), &false.into())
    );
    assert_eq!(task_read["reports"][0]["status"], "done");

    // Events name their commands in the project; storing her call keeps the reply whole.
    let events = events["read"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["caller"], "user");
    assert_eq!(gone["unavailable"], "已归档");
    backend.shutdown(Duration::from_secs(5)).await;
}

/// Angela's question waits in "等你决定" until the user's next message reaches her
/// (manager-actions.md §6 `ask_user`).
#[tokio::test]
async fn angelas_question_waits_for_the_users_next_message() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let angela = backend.backend.angela().expect("Angela opened");
    let ask = json!({ "question": "导出先做 CSV 还是 Excel？我建议先做 CSV。" });
    let asked = tell_angela(&angela, &format!("FAKE:call=ask_user{ask}"));
    let turn = angela_turn_for(&angela, &asked).await;
    let [reply] = reply_texts(&angela.db, &turn.id).try_into().expect("one reply");
    assert!(reply.starts_with("已列进「等你决定」（q_"), "{reply}");
    let open = || angela.db.read(lobotomy_core::manager::open_questions).unwrap();
    assert_eq!(open().len(), 1);

    let answer = tell_angela(&angela, "先做 CSV");
    angela_turn_for(&angela, &answer).await;
    assert!(open().is_empty(), "the answer closed it");
    backend.shutdown(Duration::from_secs(5)).await;
}

/// Angela's ledger (manager-actions.md §3): each entry in the database of its scope, a decision
/// only on the user's words, and a new session of hers starts from what holds.
#[tokio::test]
async fn angelas_ledger_is_kept_by_scope_and_starts_her_next_session() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let angela = backend.backend.angela().expect("Angela opened");
    let project = backend.project.clone();
    let said = tell_angela(&angela, "导出先做 CSV，别动公共接口");
    angela_turn_for(&angela, &said).await;

    let decision = json!({ "kind": "decision", "statement": "导出先做 CSV", "evidence": [said] });
    let constraint = json!({
        "kind": "constraint", "statement": "不改公共接口", "evidence": [said], "project_id": project.id,
    });
    let bare = json!({ "kind": "decision", "statement": "没有原话的决定" });
    let calls = [
        format!("FAKE:call=ledger_add{decision}"),
        format!("FAKE:call=ledger_add{constraint}"),
        format!("FAKE:call=ledger_add{bare}"),
        "FAKE:call=read_ledger{}".to_owned(),
    ];
    let acts = tell_angela(&angela, &calls.join("\n"));
    let turn = angela_turn_for(&angela, &acts).await;
    let [user, theirs, refused, read] = reply_texts(&angela.db, &turn.id).try_into().expect("four replies");
    assert!(user.starts_with("已记下（led_") && theirs.starts_with("已记下（led_"), "{user} / {theirs}");
    assert!(refused.contains("被拒绝"), "{refused}");
    let read: Value = serde_json::from_str(&read).unwrap();
    assert_eq!(read["user"][0]["statement"], "导出先做 CSV");
    assert_eq!(read["projects"][0]["entries"][0]["statement"], "不改公共接口");
    assert_eq!(read["projects"][0]["entries"][0]["evidence"][0]["source_id"], said.as_str());
    assert_eq!(angela.db.read(lobotomy_core::ledger::ledger).unwrap().len(), 1, "the user's scope is hers");
    assert_eq!(project.db.read(lobotomy_core::ledger::ledger).unwrap().len(), 1, "the project's is the project's");

    // She cites a decision to a role as recorded; a hypothesis never goes down (§3.4).
    let id_in = |reply: &str| reply.split(['（', '）']).nth(1).unwrap().to_owned();
    let guess = json!({ "kind": "hypothesis", "statement": "用户更在意速度", "confidence": "中" });
    let guessed = tell_angela(&angela, &format!("FAKE:call=ledger_add{guess}"));
    let turn = angela_turn_for(&angela, &guessed).await;
    let [guess] = reply_texts(&angela.db, &turn.id).try_into().expect("one reply");
    let cite = |ledger: String| json!({ "project_id": project.id, "role": "Malkuth", "interpretation": "按记录做", "ledger": [ledger] });
    let cites = format!("FAKE:call=send_to_role{}\nFAKE:call=send_to_role{}", cite(id_in(&user)), cite(id_in(&guess)));
    let cited = tell_angela(&angela, &cites);
    let turn = angela_turn_for(&angela, &cited).await;
    let [sent, refused] = reply_texts(&angela.db, &turn.id).try_into().expect("two replies");
    assert!(sent.starts_with("已放进收件箱") && refused.contains("是推测，不传给下层"), "{sent} / {refused}");
    let body: String = project
        .db
        .read(|c| Ok(c.query_row("SELECT body FROM message WHERE source = 'manager'", [], |r| r.get(0))?))
        .unwrap();
    assert!(body.starts_with("【记录】\n- [决定] 导出先做 CSV（led_"), "{body}");

    let new_session = lobotomy_core::turn::NewNativeSession { request_id: "n1".into(), role: "Angela".into() };
    angela.db.execute(&Caller::User, &new_session).unwrap();
    let session = angela.db.read(|c| lobotomy_core::turn::current_session(c, "Angela", None)).unwrap().unwrap();
    let next = tell_angela(&angela, "现在怎么样");
    angela_turn_for(&angela, &next).await;
    // The session's first turn, whatever woke her, starts from the ledger.
    let inputs: Vec<String> = angela
        .db
        .read(|c| {
            let mut stmt =
                c.prepare("SELECT input FROM turn WHERE native_session_id = ?1 ORDER BY registered_at, id")?;
            Ok(stmt.query_map([&session.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    for part in ["## 你的记录", "[决定] 导出先做 CSV", "[约束] 不改公共接口"] {
        assert!(inputs[0].contains(part), "{part} is missing from the session's first input; all inputs:\n{inputs:#?}");
    }
    backend.shutdown(Duration::from_secs(5)).await;
}

/// The user chose what a message replies to (manager-actions.md §1.1): Angela reads it, and
/// forwarding the message brings that context along though she named none; where it went shows
/// under the message ("已转给 …").
#[tokio::test]
async fn the_users_reply_target_goes_along_when_angela_forwards() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let angela = backend.backend.angela().expect("Angela opened");
    let project = backend.project.clone();
    let asked = tell_angela(&angela, "导出要不要带表头？");
    let first = angela_turn_for(&angela, &asked).await;
    let her_reply: String = angela
        .db
        .read(|c| {
            Ok(c.query_row(
                "SELECT id FROM item WHERE turn_id = ?1 AND kind = 'agent_message' ORDER BY seq DESC LIMIT 1",
                [&first.id],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    let reply = SendMessage {
        request_id: "r1".into(),
        role: "Angela".into(),
        task_id: None,
        body: "要".into(),
        reply_to: Some(her_reply.clone()),
    };
    let said = angela.db.execute(&Caller::User, &reply).unwrap().id;
    angela.wake.notify_one();
    let read = angela_turn_for(&angela, &said).await;
    assert!(read.input.contains(&format!("（回复 Angela 的 {her_reply}：")), "{}", read.input);

    let send = json!({ "project_id": project.id, "role": "Malkuth", "quote": [said], "interpretation": "表头要带上" });
    let acts = tell_angela(&angela, &format!("FAKE:call=send_to_role{send}\nFAKE:call=recent_replies{{}}"));
    let turn = angela_turn_for(&angela, &acts).await;
    let [sent, recent] = reply_texts(&angela.db, &turn.id).try_into().expect("two replies");
    assert!(sent.starts_with("已放进收件箱"), "{sent}");
    let recent: Value = serde_json::from_str(&recent).unwrap();
    assert!(recent["replies"].as_array().unwrap().iter().any(|r| r["id"] == her_reply.as_str()), "{recent}");
    let body: String = project
        .db
        .read(|c| Ok(c.query_row("SELECT body FROM message WHERE source = 'manager'", [], |r| r.get(0))?))
        .unwrap();
    assert!(body.contains("【用户当时在回复】Angela ") && body.contains(&her_reply), "{body}");
    let forwards = project.db.read(|c| lobotomy_core::view::manager::forwards(c, &said)).unwrap();
    let [forward] = forwards.as_slice() else { panic!("one forward: {forwards:?}") };
    assert_eq!((forward.kind, forward.role.as_str()), ("message", "Malkuth"));
    assert!(forward.contexts[0].chosen, "the user chose it");
    backend.shutdown(Duration::from_secs(5)).await;
}

/// An executor that says Angela's interpretation differs from the user's words reaches the user's
/// conversation with her unfiltered, while its turn still runs, and wakes her (manager-actions.md
/// §2.3).
#[tokio::test]
async fn a_mismatch_reaches_the_users_conversation_with_angela() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let angela = backend.backend.angela().expect("Angela opened");
    let project = backend.project.clone();
    wait_for("her cursor on the project", || angela.db.read(lobotomy_core::manager::cursors).unwrap().pop()).await;
    create_task(&project, "FAKE:sleep");
    let turn =
        wait_for("the executor's turn to run", || last(&project.db).filter(|t| t.state == TurnState::Running)).await;
    let flag = lobotomy_core::report::FlagMismatch { body: "原话说先别大改，解读要求重写".into() };
    project.db.execute(&Caller::Role { role: "Malkuth".into(), turn_id: turn.id }, &flag).unwrap();
    project.wake.notify_one();

    let hers = angela_turn_ended(&angela).await;
    let told = angela.db.read(|c| lobotomy_core::task::turn_messages(c, &hers.id)).unwrap();
    let mismatch = told.iter().find(|m| m.body.starts_with("⚠ ")).expect("a message of its own");
    assert_eq!(mismatch.source, "runtime");
    assert!(mismatch.body.contains("Malkuth（任务「测试任务」）：原话说先别大改，解读要求重写"), "{}", mismatch.body);
    backend.shutdown(Duration::from_secs(5)).await;
}

/// A project that settles wakes Angela (manager-actions.md §6 "Manager 什么时候运行"): the runtime
/// tells her it waits for the user, with what changed since her cursor, and she runs. The project
/// she saw first at startup, settling after its slot was written, did not wake her.
#[tokio::test]
async fn a_project_that_settles_wakes_angela_with_what_changed() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let angela = backend.backend.angela().expect("Angela opened");
    let project = backend.project.clone();
    wait_for("her cursor on the project", || angela.db.read(lobotomy_core::manager::cursors).unwrap().pop()).await;
    wait_for("the idle slot", || {
        let ready = project.db.read(|c| lobotomy_core::view::manager::settled(c, &[])).unwrap();
        ready.then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    angela.wake.notify_one();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(angela.db.read(|c| lobotomy_core::turn::last_turn(c, "Angela")).unwrap().is_none(), "no wake at startup");

    let task = create_task(&project, "FAKE:done");
    phase(&project.db, &task, Phase::Accepting).await;
    let turn = angela_turn_ended(&angela).await;
    let told = angela.db.read(|c| lobotomy_core::task::turn_messages(c, &turn.id)).unwrap();
    let [told] = told.as_slice() else { panic!("one message: {told:#?}") };
    assert_eq!(told.source, "runtime");
    let summary = told.body.lines().next().unwrap();
    assert!(summary.ends_with("全员在等你"), "{summary}");
    for part in ["现在全员在等你。", "- 等用户：", "等待验收", "汇报（完成）", task.as_str()] {
        assert!(told.body.contains(part), "{part} is missing from:\n{}", told.body);
    }
    let cursor = angela.db.read(lobotomy_core::manager::cursors).unwrap().pop().unwrap();
    assert_eq!(cursor.seq, project.db.read(lobotomy_core::view::last_event).unwrap(), "she has read it all");
    backend.shutdown(Duration::from_secs(5)).await;
}

/// Keeps a test's Claude session out of the user's Claude history: its file is
/// ~/.claude/projects/<the working directory's path>/<session>.jsonl.
fn forget_claude_session(session: &str) {
    let Some(home) = std::env::var_os("USERPROFILE").or(std::env::var_os("HOME")) else { return };
    let projects = std::path::Path::new(&home).join(".claude").join("projects");
    for project in std::fs::read_dir(&projects).into_iter().flatten().flatten() {
        let file = project.path().join(format!("{session}.jsonl"));
        if file.exists() {
            eprintln!("deleted Claude session {session}: {:?}", std::fs::remove_file(&file));
        }
    }
}

/// Angela on the real Claude, on the smallest model, calls a tool of step 2 as its schema says
/// (roadmap.md M4 step 2): she assigns a read-only task and quotes the user's message by its id.
/// Malkuth stays on the fake Codex, so only her turn uses the subscription.
#[tokio::test]
#[ignore = "runs a real Claude Code turn"]
async fn a_real_claude_angela_assigns_a_task() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = fake_codex();
    harness.claude = real_clis().claude;
    let backend = start_with(dir.path(), harness).await;
    let angela = backend.backend.angela().expect("Angela opened");
    angela.db.write(|tx| Ok(tx.execute("UPDATE role SET model = 'haiku' WHERE name = 'Angela'", [])?)).unwrap();
    let project = backend.project.clone();
    let ask = format!(
        "请在项目 {} 里给 Malkuth 派一个只读调查任务：看看 README 里写了什么。用 quote 引用我这条消息。\
         停止条件：读完 README 就汇报。完成条件：说明 README 的内容。直接派，不用先问我。",
        project.id
    );
    let said = tell_angela(&angela, &ask);
    let turn = angela_turn_for(&angela, &said).await;
    let calls: Vec<String> = angela
        .db
        .read(|c| {
            let mut stmt =
                c.prepare("SELECT content FROM item WHERE turn_id = ?1 AND kind = 'mcp_call' ORDER BY seq")?;
            Ok(stmt.query_map([&turn.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    eprintln!("outcome {:?}, failure {:?}", turn.outcome, turn.failure);
    for call in &calls {
        eprintln!("call {}", &call[..call.len().min(600)]);
    }
    if let Some(session) = &turn.native_id {
        forget_claude_session(session);
    }

    assert_eq!((turn.harness.as_str(), turn.outcome), ("claude", Some(Outcome::Completed)));
    let tasks = project.db.read(lobotomy_core::task::list_tasks).unwrap();
    let task = tasks.iter().find(|t| t.assignment.is_some()).expect("she assigned a task");
    let assignment = task.assignment.as_ref().unwrap();
    assert_eq!(assignment.actions, [lobotomy_core::task::Action::ReadOnly]);
    assert!(!assignment.stop_when.trim().is_empty());
    let sources: Vec<String> = project
        .db
        .read(|c| {
            let mut stmt = c.prepare("SELECT source_id FROM quote WHERE task_id = ?1")?;
            Ok(stmt.query_map([&task.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    assert_eq!(sources, [said], "she quoted the user's message by its id");
    backend.shutdown(Duration::from_secs(5)).await;
}

/// The texts of the Lobotomy tools' replies in a turn, in order.
fn reply_texts(db: &Db, turn: &str) -> Vec<String> {
    db.read(|c| {
        let mut stmt = c.prepare("SELECT content FROM item WHERE turn_id = ?1 AND kind = 'mcp_call' ORDER BY seq")?;
        let rows = stmt.query_map([turn], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .unwrap()
    .into_iter()
    .map(|content| serde_json::from_str::<Value>(&content).unwrap()["result_text"].as_str().unwrap().to_owned())
    .collect()
}

/// Angela acts in a project as `manager` (roadmap.md M4 step 2): her task carries the user's words
/// she quoted by id, copied from her database with that id (data-model.md §10.6 跨库操作第 1 条);
/// the user's pause stays until the user lifts it (roles-and-tasks.md §4).
#[tokio::test]
async fn angela_assigns_with_the_users_words_and_leaves_the_users_pause_alone() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let angela = backend.backend.angela().expect("Angela opened");
    let project = backend.project.clone();
    let said = tell_angela(&angela, "启动太慢了，先看看为什么");
    angela_turn_for(&angela, &said).await;
    let held = create_task(&project, "先别动");
    project
        .db
        .execute(&Caller::User, &SetPaused { request_id: "p1".into(), task_id: held.clone(), paused: true })
        .unwrap();

    let assign = json!({
        "project_id": project.id, "role": "Malkuth", "title": "查清启动慢的原因", "quote": [said],
        "interpretation": "先定位，不改代码", "actions": ["read_only"],
        "stop_when": "找到第一个确定的原因就汇报", "done_when": "说明原因和证据",
    });
    let resume = json!({ "project_id": project.id, "task_id": held, "paused": false });
    let acts = tell_angela(&angela, &format!("FAKE:call=assign{assign}\nFAKE:call=set_paused{resume}"));
    let turn = angela_turn_for(&angela, &acts).await;
    let [assigned, refused] = reply_texts(&angela.db, &turn.id).try_into().expect("two replies");
    assert!(assigned.starts_with("已派出任务 task_"), "{assigned}");
    assert!(refused.contains("只有用户能恢复"), "{refused}");
    assert!(project.db.read(|c| load_task(c, &held)).unwrap().paused);

    let tasks = project.db.read(lobotomy_core::task::list_tasks).unwrap();
    let task = tasks.iter().find(|t| t.assignment.is_some()).expect("her task");
    assert!(task.body.contains("启动太慢了，先看看为什么"), "{}", task.body);
    let (source, caller): (String, String) = project
        .db
        .read(|c| {
            Ok(c.query_row(
                "SELECT q.source_id, r.caller FROM quote q, command_record r WHERE q.task_id = ?1 AND r.name = 'assign'",
                [&task.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!((source.as_str(), caller.as_str()), (said.as_str(), "manager"));
    backend.shutdown(Duration::from_secs(5)).await;
}
