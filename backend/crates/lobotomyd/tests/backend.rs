//! The backend end to end with a fake Codex and a fake Claude (tests/fixtures/fake-*.mjs, run by
//! Node): the scheduler, the turn runner, the MCP service and the database together. Needs `node`
//! on PATH.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use lobotomy_core::id::now_ms;
use lobotomy_core::task::{Abandon, SendMessage};
use lobotomy_core::turn::{FailureKind, Outcome, SetRoleHarness, TurnState, hold, load_turn};
use lobotomy_core::{Caller, Db};
use lobotomyd::host::Host;
use lobotomyd::instance::Instance;
use lobotomyd::project::Project;
use serde_json::{Value, json};

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

/// An idle backend with no GUI clients or turns releases its project on shutdown,
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
    let caller = Caller::Role { role: "Malkuth".into(), turn_id: turn.id };
    project.db.execute(&caller, &flag).unwrap();
    project.wake.notify_one();

    let hers = angela_turn_ended(&angela).await;
    let told = angela.db.read(|c| lobotomy_core::task::turn_messages(c, &hers.id)).unwrap();
    let mismatch = told.iter().find(|m| m.body.starts_with("⚠ ")).expect("a message of its own");
    assert_eq!(mismatch.source, "runtime");
    assert!(mismatch.body.contains("Malkuth（任务「测试任务」）：原话说先别大改，解读要求重写"), "{}", mismatch.body);

    // Holding Angela after a failure must not hide the executor's next notice from the user.
    let failed = angela_turn_for(&angela, &tell_angela(&angela, "FAKE:fail")).await;
    assert_eq!(failed.outcome, Some(lobotomy_core::turn::Outcome::Failed));
    let flag = lobotomy_core::report::FlagMismatch { body: "暂停时仍要告知用户".into() };
    project.db.execute(&caller, &flag).unwrap();
    project.wake.notify_one();
    let notice = wait_for("the notice while Angela holds", || {
        angela
            .db
            .read(|c| lobotomy_core::task::queued_messages(c, "Angela"))
            .unwrap()
            .into_iter()
            .find(|m| m.body.starts_with("⚠ ") && m.body.contains("暂停时仍要告知用户"))
    })
    .await;
    assert_eq!(notice.source, "runtime");
    assert_eq!(angela.db.read(|c| lobotomy_core::turn::last_turn(c, "Angela")).unwrap().unwrap().id, failed.id);
    let cursor = angela.db.read(lobotomy_core::manager::cursors).unwrap().pop().unwrap();
    assert_eq!(cursor.seq, project.db.read(lobotomy_core::view::last_event).unwrap());
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
