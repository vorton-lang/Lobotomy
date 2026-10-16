use lobotomy_core::item::{BlobStore, STORAGE_THRESHOLD, externalize};
use lobotomy_core::report::{ReportEffect, ReportStatus};
use lobotomy_core::task::{Abandon, CreateTask, EditCriteria, Reopen, SendBack, StartAttempt, load_task};
use lobotomy_core::turn::{
    Continue, EndTurn, NewNativeSession, Outcome, RegisterTurn, SessionIdentified, TurnRegistered, current_session,
    load_turn,
};
use lobotomy_core::view::{Attention, attention, overview};
use lobotomy_core::{Caller, Db};
use serde_json::json;

mod common;
use common::{ROLE, db, rejection, report, start};

/// Creates a task and starts its first attempt; the brief waits in the inbox.
fn started_task(db: &Db) -> String {
    let id = db
        .execute(
            &Caller::User,
            &CreateTask {
                request_id: "c1".into(),
                title: "实现 adapter".into(),
                body: "把 Codex 接进来".into(),
                criteria: "测试通过".into(),
                executor: ROLE.into(),
            },
        )
        .unwrap()
        .id;
    start(db, &id);
    id
}

/// Registers a turn without identifying its harness session: these tests see what happens before
/// `thread.started`, or without it (#10). `common::register` identifies it.
fn register(db: &Db) -> lobotomy_core::Result<TurnRegistered> {
    db.execute(&Caller::Runtime, &RegisterTurn { role: ROLE.into() })
}

fn send(db: &Db, request_id: &str, body: &str) {
    common::send(db, request_id, None, body);
}

fn end(db: &Db, turn_id: &str, outcome: Outcome) {
    db.execute(&Caller::Runtime, &EndTurn { turn_id: turn_id.into(), outcome, failure: None }).unwrap();
}

fn create_named(db: &Db, request_id: &str, title: &str) -> String {
    db.execute(
        &Caller::User,
        &CreateTask {
            request_id: request_id.into(),
            title: title.into(),
            body: "原话".into(),
            criteria: "测试通过".into(),
            executor: ROLE.into(),
        },
    )
    .unwrap()
    .id
}

fn abandon(db: &Db, request_id: &str, task: &str) {
    db.execute(&Caller::User, &Abandon { request_id: request_id.into(), task_id: task.into(), reason: String::new() })
        .unwrap();
}

#[test]
fn abandoning_after_an_abnormal_turn_lets_the_next_task_run_in_a_fresh_session() {
    // Issue #11.
    for outcome in [Outcome::Interrupted, Outcome::Failed] {
        let db = db();
        let a = started_task(&db);
        let b = create_named(&db, "c2", "下一个任务");
        let t = register(&db).unwrap();
        end(&db, &t.turn_id, outcome);
        abandon(&db, "x1", &a);
        assert!(db.read(|c| current_session(c, ROLE, Some(&a))).unwrap().is_none(), "A's session ended");

        start(&db, &b);
        let next = register(&db).unwrap();
        let next = db.read(|c| load_turn(c, &next.turn_id)).unwrap();
        assert_eq!(next.task_id.as_deref(), Some(b.as_str()));
        assert_ne!(next.native_session_id, t_session(&db, &t.turn_id), "B runs in a fresh session");
        // A's abnormal turn is not B's to continue.
        end(&db, &next.id, Outcome::Completed);
        let again = db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: ROLE.into() });
        assert_eq!(rejection(again), "nothing_to_continue");
    }
}

fn t_session(db: &Db, turn_id: &str) -> String {
    db.read(|c| load_turn(c, turn_id)).unwrap().native_session_id
}

#[test]
fn a_reopened_task_starts_in_a_fresh_session() {
    let db = db();
    let a = started_task(&db);
    let t = register(&db).unwrap();
    end(&db, &t.turn_id, Outcome::Interrupted);
    abandon(&db, "x1", &a);
    db.execute(&Caller::User, &Reopen { request_id: "o1".into(), task_id: a.clone() }).unwrap();
    start(&db, &a);
    let next = register(&db).unwrap();
    assert_ne!(t_session(&db, &next.turn_id), t_session(&db, &t.turn_id));
}

#[test]
fn a_running_turn_of_an_abandoned_task_holds_the_next_tasks_turn() {
    let db = db();
    let a = started_task(&db);
    let b = create_named(&db, "c2", "下一个任务");
    let t = register(&db).unwrap();
    // A's CLI still runs when A is abandoned. B starts, but its turn waits for A's.
    abandon(&db, "x1", &a);
    db.execute(&Caller::Runtime, &StartAttempt { task_id: b.clone() }).unwrap();
    assert_eq!(rejection(register(&db)), "turn_unfinished");
    // The database allows one unfinished turn per role, whatever the session.
    let second = db.write(|c| {
        Ok(c.execute(
            "INSERT INTO native_session (id, role, harness, started_at) VALUES ('ns_other', 'Malkuth', 'codex', 0);
             ",
            [],
        )
        .and_then(|_| {
            c.execute(
                "INSERT INTO turn (id, role, native_session_id, token, input, state, registered_at)
                 VALUES ('turn_dup', 'Malkuth', 'ns_other', 'tok_dup', '', 'registered', 0)",
                [],
            )
        }))
    });
    assert!(second.unwrap().is_err());
    // Once A's turn ends, B's runs.
    end(&db, &t.turn_id, Outcome::Completed);
    let next = register(&db).unwrap();
    assert_eq!(db.read(|c| load_turn(c, &next.turn_id)).unwrap().task_id.as_deref(), Some(b.as_str()));
}

#[test]
fn an_abnormal_turn_of_an_ended_attempt_does_not_hold_the_next_attempt() {
    let db = db();
    let task = started_task(&db);
    let t = register(&db).unwrap();
    db.execute(&Caller::Runtime, &SessionIdentified { turn_id: t.turn_id.clone(), native_id: "thread-1".into() })
        .unwrap();
    report(&db, &t.turn_id, ReportStatus::Done, None).unwrap();
    // The turn reported done, then ended abnormally: the attempt ended with its candidate.
    end(&db, &t.turn_id, Outcome::Interrupted);
    // Sent back: a new attempt opens.
    let send_back = SendBack { request_id: "b1".into(), task_id: task.clone(), reason: "补上测试".into() };
    db.execute(&Caller::User, &send_back).unwrap();
    let next = register(&db).unwrap();
    assert_eq!(t_session(&db, &next.turn_id), t_session(&db, &t.turn_id), "same task, same session");
    assert!(db.read(|c| load_turn(c, &next.turn_id)).unwrap().input.contains("补上测试"));
}

#[test]
fn a_completed_session_without_a_harness_id_stops_the_role() {
    let db = db();
    started_task(&db);
    let t = register(&db).unwrap();
    // The turn completed, but recording the session id failed.
    end(&db, &t.turn_id, Outcome::Completed);
    send(&db, "m1", "继续");
    assert_eq!(rejection(register(&db)), "session_unidentified");
    let again = db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: ROLE.into() });
    assert_eq!(rejection(again), "nothing_to_continue");
    // A new native session is the way on.
    db.execute(&Caller::User, &NewNativeSession { request_id: "n1".into(), role: ROLE.into() }).unwrap();
    register(&db).unwrap();
}

/// The executor's question waits for the user until a message from the user is bound to its next
/// turn (#17). A queued reply already takes the question off the user's list; the bound reply
/// clears it. A message from the runtime is no reply, and asking again brings the question back.
#[test]
fn a_reply_bound_to_the_next_turn_clears_the_question() {
    let db = db();
    let task = started_task(&db);
    let reason = || db.read(|c| load_task(c, &task)).unwrap().blocked_reason;
    let asks_user = || {
        let list = db.read(|c| Ok(attention(&overview(c, "lobotomy")?, &[], |_| false))).unwrap();
        list.iter().any(|a| matches!(a, Attention::TaskBlocked { .. }))
    };
    let t1 = common::register(&db);
    report(&db, &t1.turn_id, ReportStatus::Blocked, Some("空值怎么处理？")).unwrap();
    assert!(asks_user());
    end(&db, &t1.turn_id, Outcome::Completed);

    let text = "空值报错".into();
    db.execute(
        &Caller::User,
        &EditCriteria { request_id: "e1".into(), task_id: task.clone(), expected_version: 1, text },
    )
    .unwrap();
    let t2 = common::register(&db);
    assert_eq!(reason().as_deref(), Some("空值怎么处理？"), "the criteria update is not a reply");
    assert!(asks_user());
    end(&db, &t2.turn_id, Outcome::Completed);

    common::send(&db, "m1", Some(&task), "空值报错");
    assert!(!asks_user(), "the reply waits for the next turn; the user has nothing to do");
    assert_eq!(reason().as_deref(), Some("空值怎么处理？"), "a queued reply does not clear the question");
    let t3 = common::register(&db);
    assert_eq!(reason(), None);
    assert!(!asks_user());

    let again = report(&db, &t3.turn_id, ReportStatus::Blocked, Some("空值怎么处理？")).unwrap();
    assert_eq!(again, ReportEffect::Recorded);
    assert!(asks_user());
}

/// A question asked before the task was abandoned does not come back when it is reopened: the
/// GUI would show it as the executor waiting for the user (#16).
#[test]
fn a_question_does_not_outlive_its_task() {
    let db = db();
    let task = started_task(&db);
    let t = register(&db).unwrap();
    report(&db, &t.turn_id, ReportStatus::Blocked, Some("用哪个检查命令？")).unwrap();
    end(&db, &t.turn_id, Outcome::Completed);
    let abandon = Abandon { request_id: "a1".into(), task_id: task.clone(), reason: "先不做".into() };
    db.execute(&Caller::User, &abandon).unwrap();
    let reason = || db.read(|c| load_task(c, &task)).unwrap().blocked_reason;
    assert!(db.read(|c| lobotomy_core::question::open_questions(c, Some(ROLE))).unwrap().is_empty());
    assert_eq!(reason(), None, "a closed task asks nothing");
    db.execute(&Caller::User, &Reopen { request_id: "r1".into(), task_id: task.clone() }).unwrap();
    start(&db, &task);
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().phase, lobotomy_core::task::Phase::Executing);
    assert!(db.read(|c| lobotomy_core::question::open_questions(c, Some(ROLE))).unwrap().is_empty());
    assert_eq!(reason(), None);
}

#[test]
fn a_field_whose_blob_cannot_be_written_stays_in_the_item() {
    let dir = tempfile::tempdir().unwrap();
    // A file where the blob directory should be: every blob write fails.
    let blocked = dir.path().join("blobs");
    std::fs::write(&blocked, "not a directory").unwrap();
    let store = BlobStore::new(&blocked);
    let output = "x".repeat(STORAGE_THRESHOLD + 1);
    let mut content = json!({ "command": "cargo build", "output": output });
    assert!(externalize(&store, &mut content).is_empty());
    assert_eq!(content["output"], output, "the full text stays in the content");
}
