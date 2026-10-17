//! The backend end to end with a fake Codex and a fake Claude (tests/fixtures/fake-*.mjs, run by
//! Node): Angela's turns, her MCP service, the projects and the database together. Executors run in
//! the worker hosts; their runner's tests are the worker's (`crates/worker/tests`). Needs `node` on
//! PATH.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use lobotomy_core::id::now_ms;
use lobotomy_core::role::SyncWorkerRoles;
use lobotomy_core::task::{CreateTask, SendMessage};
use lobotomy_core::turn::{Outcome, SetRoleHarness, TurnState, load_turn};
use lobotomy_core::worker::{ApplyWorkerEvent, WorkerTaskCreated};
use lobotomy_core::{Caller, Db};
use lobotomy_worker_protocol::event::{Event, EventBody};
use lobotomyd::host::Host;
use lobotomyd::instance::Instance;
use lobotomyd::project::Project;
use serde_json::{Value, json};

/// A worker task in the project, as if a worker named `w` had taken it: the role, the task and
/// the worker's record of it, without a worker host.
fn worker_task(project: &Project) -> String {
    let db = &project.db;
    db.execute(&Caller::Runtime, &SyncWorkerRoles { workers: vec![("w".into(), "codex".into())] }).unwrap();
    let create = CreateTask {
        request_id: "t".into(),
        title: "测试任务".into(),
        body: "原话".into(),
        criteria: "-".into(),
        executor: "w".into(),
    };
    let task = db.execute(&Caller::User, &create).unwrap().id;
    let taken =
        WorkerTaskCreated { task_id: task.clone(), worker: "w".into(), spec_version: 1, attempt_id: "a1".into() };
    db.execute(&Caller::Runtime, &taken).unwrap();
    task
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

/// The arguments of the fake CLI's last run in `dir`.
fn last_args(dir: &std::path::Path) -> Vec<String> {
    serde_json::from_str(&std::fs::read_to_string(diag(dir).join("last-args.json")).unwrap()).unwrap()
}

fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
    &args[args.iter().position(|a| a == flag).unwrap_or_else(|| panic!("no {flag} in {args:?}")) + 1]
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
        stop_id: None,
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

    // She cites a decision to a worker as recorded; a hypothesis never goes down (§3.4).
    let id_in = |reply: &str| reply.split(['（', '）']).nth(1).unwrap().to_owned();
    let guess = json!({ "kind": "hypothesis", "statement": "用户更在意速度", "confidence": "中" });
    let guessed = tell_angela(&angela, &format!("FAKE:call=ledger_add{guess}"));
    let turn = angela_turn_for(&angela, &guessed).await;
    let [guess] = reply_texts(&angela.db, &turn.id).try_into().expect("one reply");
    project.db.execute(&Caller::Runtime, &SyncWorkerRoles { workers: vec![("w".into(), "codex".into())] }).unwrap();
    let cite = |ledger: String| {
        json!({
            "project_id": project.id, "role": "w", "title": "按记录做", "interpretation": "按记录做",
            "actions": ["read_only"], "stop_when": "读完就停", "done_when": "-", "ledger": [ledger],
        })
    };
    let cites = format!("FAKE:call=assign{}\nFAKE:call=assign{}", cite(id_in(&user)), cite(id_in(&guess)));
    let cited = tell_angela(&angela, &cites);
    let turn = angela_turn_for(&angela, &cited).await;
    let [sent, refused] = reply_texts(&angela.db, &turn.id).try_into().expect("two replies");
    assert!(sent.starts_with("已派出任务") && refused.contains("是推测，不传给下层"), "{sent} / {refused}");
    let body: String =
        project.db.read(|c| Ok(c.query_row("SELECT body FROM task WHERE executor = 'w'", [], |r| r.get(0))?)).unwrap();
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

/// A worker that says Angela's interpretation differs from the user's words reaches the user's
/// conversation with her unfiltered, and wakes her (manager-actions.md §2.3; worker-interface.md
/// §5).
#[tokio::test]
async fn a_mismatch_reaches_the_users_conversation_with_angela() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let angela = backend.backend.angela().expect("Angela opened");
    let project = backend.project.clone();
    wait_for("her cursor on the project", || angela.db.read(lobotomy_core::manager::cursors).unwrap().pop()).await;
    let task = worker_task(&project);
    let flag = |seq: u64, body: &str| ApplyWorkerEvent {
        task_id: task.clone(),
        event: Event {
            task_id: task.clone(),
            attempt_id: "a1".into(),
            seq,
            body: EventBody::MismatchFlagged { body: body.into() },
        },
    };
    project.db.execute(&Caller::Runtime, &flag(1, "原话说先别大改，解读要求重写")).unwrap();
    project.wake.notify_one();

    let hers = angela_turn_ended(&angela).await;
    let told = angela.db.read(|c| lobotomy_core::task::turn_messages(c, &hers.id)).unwrap();
    let mismatch = told.iter().find(|m| m.body.starts_with("⚠ ")).expect("a message of its own");
    assert_eq!(mismatch.source, "runtime");
    assert!(mismatch.body.contains("w（任务「测试任务」）：原话说先别大改，解读要求重写"), "{}", mismatch.body);

    // Holding Angela after a failure must not hide the executor's next notice from the user.
    let failed = angela_turn_for(&angela, &tell_angela(&angela, "FAKE:fail")).await;
    assert_eq!(failed.outcome, Some(lobotomy_core::turn::Outcome::Failed));
    project.db.execute(&Caller::Runtime, &flag(2, "暂停时仍要告知用户")).unwrap();
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
