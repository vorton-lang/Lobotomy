use lobotomy_core::item::{BlobStore, NewItem, STORAGE_THRESHOLD, externalize, record_item};
use lobotomy_core::report::{OrgReport, ReportEffect, ReportStatus};
use lobotomy_core::task::{
    Abandon, CreateTask, Reopen, SendMessage, StartAttempt, load_task, open_attempt, queued_messages,
};
use lobotomy_core::turn::{
    Continue, EndTurn, Failure, FailureKind, InputDelivered, MarkUnfinishedUnknown, NewNativeSession, Outcome,
    RegisterTurn, SessionIdentified, TurnLaunched, TurnRegistered, TurnState, current_session, load_turn,
    turn_by_token,
};
use lobotomy_core::verify::{CheckOutcome, FinishVerification, StartVerification};
use lobotomy_core::{Caller, Db};
use serde_json::json;

mod common;
use common::{ROLE, db, pin_captures, rejection, start};

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

fn register(db: &Db) -> lobotomy_core::Result<TurnRegistered> {
    db.execute(&Caller::Runtime, &RegisterTurn { role: ROLE.into() })
}

fn send(db: &Db, request_id: &str, body: &str) {
    db.execute(
        &Caller::User,
        &SendMessage { request_id: request_id.into(), role: ROLE.into(), task_id: None, body: body.into() },
    )
    .unwrap();
}

/// Ends the turn and lets the store capture the slot.
fn end(db: &Db, turn_id: &str, outcome: Outcome) {
    db.execute(&Caller::Runtime, &EndTurn { turn_id: turn_id.into(), outcome, failure: None }).unwrap();
    pin_captures(db);
}

fn report(
    db: &Db,
    turn_id: &str,
    status: ReportStatus,
    blocked_on: Option<&str>,
) -> lobotomy_core::Result<ReportEffect> {
    db.execute(
        &Caller::Role { role: ROLE.into(), turn_id: turn_id.into() },
        &OrgReport { title: "汇报".into(), body: "内容".into(), status, blocked_on: blocked_on.map(Into::into) },
    )
}

#[test]
fn registering_a_turn_binds_the_brief_and_opens_a_session() {
    let db = db();
    let task = started_task(&db);
    let t = register(&db).unwrap();
    let turn = db.read(|c| load_turn(c, &t.turn_id)).unwrap();
    assert_eq!(turn.state, TurnState::Registered);
    assert_eq!(turn.task_id.as_deref(), Some(task.as_str()));
    assert!(turn.input.contains("实现 adapter") && turn.input.contains("测试通过"), "{}", turn.input);
    assert!(db.read(|c| queued_messages(c, ROLE)).unwrap().is_empty());
    assert!(db.read(|c| current_session(c, ROLE, Some(&task))).unwrap().is_some());
    assert_eq!(db.read(|c| turn_by_token(c, &t.token)).unwrap().unwrap().id, t.turn_id);
}

#[test]
fn a_session_runs_one_turn_at_a_time() {
    let db = db();
    started_task(&db);
    let t = register(&db).unwrap();
    send(&db, "m1", "再看一下");
    assert_eq!(rejection(register(&db)), "turn_unfinished");
    // The database enforces it too.
    let duplicate = db.read(|c| {
        Ok(c.execute(
            "INSERT INTO turn (id, role, native_session_id, token, input, state, registered_at)
             SELECT 'turn_dup', role, native_session_id, 'tok_dup', '', 'registered', 0 FROM turn WHERE id = ?1",
            [&t.turn_id],
        ))
    });
    assert!(duplicate.unwrap().is_err());
}

#[test]
fn a_turn_runs_through_its_states_and_delivers_its_messages() {
    let db = db();
    started_task(&db);
    let t = register(&db).unwrap();
    db.execute(&Caller::Runtime, &TurnLaunched { turn_id: t.turn_id.clone(), pid: 42, process_start: 7 }).unwrap();
    db.execute(&Caller::Runtime, &InputDelivered { turn_id: t.turn_id.clone() }).unwrap();
    let undelivered: i64 = db
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM message WHERE delivered_at IS NULL", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(undelivered, 0);
    db.execute(&Caller::Runtime, &SessionIdentified { turn_id: t.turn_id.clone(), native_id: "thread-1".into() })
        .unwrap();
    let other = SessionIdentified { turn_id: t.turn_id.clone(), native_id: "thread-2".into() };
    assert_eq!(rejection(db.execute(&Caller::Runtime, &other)), "native_id_mismatch");
    end(&db, &t.turn_id, Outcome::Completed);
    let turn = db.read(|c| load_turn(c, &t.turn_id)).unwrap();
    assert_eq!((turn.state, turn.outcome, turn.pid), (TurnState::Ended, Some(Outcome::Completed), Some(42)));
    assert_eq!(turn.native_id.as_deref(), Some("thread-1"));

    // Nothing queued, nothing to run; a new message starts the next turn in the same session.
    assert_eq!(rejection(register(&db)), "nothing_to_deliver");
    send(&db, "m1", "补一个测试");
    let next = register(&db).unwrap();
    let next = db.read(|c| load_turn(c, &next.turn_id)).unwrap();
    assert_eq!(next.native_session_id, turn.native_session_id);
    assert!(next.input.contains("补一个测试"));
}

#[test]
fn after_an_abnormal_turn_the_role_waits_for_continue() {
    let db = db();
    started_task(&db);
    let t = register(&db).unwrap();
    end(&db, &t.turn_id, Outcome::Interrupted);
    send(&db, "m1", "先别管测试");
    assert_eq!(rejection(register(&db)), "held");

    let next = db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: ROLE.into() }).unwrap();
    let input = db.read(|c| load_turn(c, &next.turn_id)).unwrap().input;
    let note = input.find("被中断").expect("the note");
    let message = input.find("先别管测试").expect("the queued message");
    assert!(note < message, "the note comes first: {input}");

    // Continue needs an interrupted or failed turn.
    end(&db, &next.turn_id, Outcome::Completed);
    let again = db.execute(&Caller::User, &Continue { request_id: "k2".into(), role: ROLE.into() });
    assert_eq!(rejection(again), "nothing_to_continue");
}

#[test]
fn a_quota_failure_is_named_in_the_continue_note() {
    let db = db();
    started_task(&db);
    let t = register(&db).unwrap();
    let failure = Failure { kind: FailureKind::Quota, message: "limit".into(), resets_at: Some(1) };
    db.execute(&Caller::Runtime, &EndTurn { turn_id: t.turn_id, outcome: Outcome::Failed, failure: Some(failure) })
        .unwrap();
    pin_captures(&db);
    // A quota failure holds the role like any failure; only the user continues (data-model.md §8.5).
    send(&db, "m1", "额度恢复了");
    assert_eq!(rejection(register(&db)), "held");
    let by_runtime = db.execute(&Caller::Runtime, &Continue { request_id: "k0".into(), role: ROLE.into() });
    assert_eq!(rejection(by_runtime), "forbidden");
    let next = db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: ROLE.into() }).unwrap();
    assert!(db.read(|c| load_turn(c, &next.turn_id)).unwrap().input.contains("额度不足"));
}

#[test]
fn a_new_native_session_releases_the_hold_and_resends_the_brief() {
    let db = db();
    let task = started_task(&db);
    let t = register(&db).unwrap();
    end(&db, &t.turn_id, Outcome::Interrupted);
    let old = db.read(|c| current_session(c, ROLE, Some(&task))).unwrap().unwrap();
    db.execute(&Caller::User, &NewNativeSession { request_id: "n1".into(), role: ROLE.into() }).unwrap();
    let new = db.read(|c| current_session(c, ROLE, Some(&task))).unwrap().unwrap();
    assert_ne!(old.id, new.id);
    let next = register(&db).unwrap();
    let input = db.read(|c| load_turn(c, &next.turn_id)).unwrap().input;
    assert!(input.contains("（新会话）") && input.contains("实现 adapter"), "{input}");
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
fn a_running_turn_of_an_abandoned_task_blocks_the_next_task() {
    let db = db();
    let a = started_task(&db);
    let b = create_named(&db, "c2", "下一个任务");
    let t = register(&db).unwrap();
    // A's CLI still runs when A is abandoned. B cannot take the slot while it runs.
    abandon(&db, "x1", &a);
    let start_b = StartAttempt { task_id: b.clone(), code_start: None };
    assert_eq!(rejection(db.execute(&Caller::Runtime, &start_b)), "turn_unfinished");
    // The database allows one unfinished turn per role, whatever the session.
    let second = db.read(|c| {
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
    // Once A's turn ends and its scene is captured, B starts and runs.
    end(&db, &t.turn_id, Outcome::Completed);
    start(&db, &b);
    register(&db).unwrap();
}

#[test]
fn an_abnormal_turn_of_an_ended_attempt_does_not_hold_the_next_attempt() {
    let db = db();
    let task = started_task(&db);
    let t = register(&db).unwrap();
    db.execute(&Caller::Runtime, &SessionIdentified { turn_id: t.turn_id.clone(), native_id: "thread-1".into() })
        .unwrap();
    report(&db, &t.turn_id, ReportStatus::Done, None).unwrap();
    // The turn reported done, then ended abnormally; its capture became the candidate.
    let [commit] = <[String; 1]>::try_from({
        db.execute(
            &Caller::Runtime,
            &EndTurn { turn_id: t.turn_id.clone(), outcome: Outcome::Interrupted, failure: None },
        )
        .unwrap();
        pin_captures(&db)
    })
    .unwrap();
    // Verification failed: the attempt ended with its candidate and a new one opens.
    let v = db.execute(&Caller::Runtime, &StartVerification { task_id: task.clone() }).unwrap();
    let check = CheckOutcome {
        command: "cargo test".into(),
        exit_code: Some(101),
        timed_out: false,
        output: json!({ "text": "1 failed" }),
        duration_ms: 10,
        tail: "1 failed".into(),
    };
    let finish =
        FinishVerification { verification_id: v, commit, conflicts: vec![], checks: vec![check], blobs: vec![] };
    db.execute(&Caller::Runtime, &finish).unwrap();
    let next = register(&db).unwrap();
    assert_eq!(t_session(&db, &next.turn_id), t_session(&db, &t.turn_id), "same task, same session");
    assert!(db.read(|c| load_turn(c, &next.turn_id)).unwrap().input.contains("cargo test"));
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

#[test]
fn messages_wait_while_the_task_is_outside_execution_or_paused() {
    let db = db();
    let task = started_task(&db);
    db.read(|c| Ok(c.execute("UPDATE task SET phase = 'verifying' WHERE id = ?1", [&task])?)).unwrap();
    assert_eq!(rejection(register(&db)), "not_executing");
    db.read(|c| Ok(c.execute("UPDATE task SET phase = 'executing', paused = 1 WHERE id = ?1", [&task])?)).unwrap();
    assert_eq!(rejection(register(&db)), "paused");
}

#[test]
fn startup_marks_unfinished_turns_unknown_until_reconciled() {
    let db = db();
    started_task(&db);
    let t = register(&db).unwrap();
    let marked = db.execute(&Caller::Runtime, &MarkUnfinishedUnknown {}).unwrap();
    assert_eq!(marked, [t.turn_id.as_str()]);
    send(&db, "m1", "还在吗");
    assert_eq!(rejection(register(&db)), "turn_unfinished");
    // Reconciliation found the CLI gone.
    end(&db, &t.turn_id, Outcome::Interrupted);
    assert_eq!(rejection(register(&db)), "held");
}

#[test]
fn done_is_recorded_once_and_its_capture_moves_the_task_to_verification() {
    let db = db();
    let task = started_task(&db);
    let t = register(&db).unwrap();
    assert_eq!(report(&db, &t.turn_id, ReportStatus::Done, None).unwrap(), ReportEffect::Recorded);
    assert_eq!(report(&db, &t.turn_id, ReportStatus::Done, None).unwrap(), ReportEffect::Unchanged);
    let attempt = db.read(|c| open_attempt(c, &task)).unwrap().unwrap();
    assert_eq!(attempt.done_turn_id.as_deref(), Some(t.turn_id.as_str()));
    db.execute(&Caller::Runtime, &SessionIdentified { turn_id: t.turn_id.clone(), native_id: "thread-1".into() })
        .unwrap();
    db.execute(&Caller::Runtime, &EndTurn { turn_id: t.turn_id.clone(), outcome: Outcome::Completed, failure: None })
        .unwrap();
    // Nothing runs in the slot before its scene is captured.
    send(&db, "m1", "还有一件事");
    assert_eq!(rejection(register(&db)), "capture_pending");
    pin_captures(&db);
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().phase, lobotomy_core::task::Phase::Verifying);
    assert_eq!(db.read(|c| open_attempt(c, &task)).unwrap(), None, "the attempt ended with its candidate");
    assert_eq!(rejection(register(&db)), "not_executing");
}

#[test]
fn blocked_sets_the_reason_once_and_progress_clears_it() {
    let db = db();
    let task = started_task(&db);
    let t = register(&db).unwrap();
    let blocked = |reason| report(&db, &t.turn_id, ReportStatus::Blocked, Some(reason)).unwrap();
    assert_eq!(blocked("用哪个检查命令？"), ReportEffect::Recorded);
    assert_eq!(blocked("用哪个检查命令？"), ReportEffect::Unchanged);
    let reason = |db: &Db| db.read(|c| load_task(c, &task)).unwrap().blocked_reason;
    assert_eq!(reason(&db).as_deref(), Some("用哪个检查命令？"));
    report(&db, &t.turn_id, ReportStatus::Progress, None).unwrap();
    assert_eq!(reason(&db), None);
}

#[test]
fn reports_after_the_turn_ended_are_late_and_change_nothing() {
    let db = db();
    let task = started_task(&db);
    let t = register(&db).unwrap();
    end(&db, &t.turn_id, Outcome::Completed);
    assert_eq!(report(&db, &t.turn_id, ReportStatus::Done, None).unwrap(), ReportEffect::Late);
    assert_eq!(db.read(|c| open_attempt(c, &task)).unwrap().unwrap().done_turn_id, None);
    // The late call is still on record.
    let recorded: i64 = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM command_record WHERE name = 'org_report' AND turn_id = ?1",
                [&t.turn_id],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(recorded, 1);
}

#[test]
fn only_the_turns_own_role_can_report() {
    let db = db();
    started_task(&db);
    let t = register(&db).unwrap();
    let other = Caller::Role { role: "Yesod".into(), turn_id: t.turn_id.clone() };
    let r = db.execute(
        &other,
        &OrgReport { title: "x".into(), body: "x".into(), status: ReportStatus::Done, blocked_on: None },
    );
    assert_eq!(rejection(r), "forbidden");
    let r = db.execute(
        &Caller::User,
        &OrgReport { title: "x".into(), body: "x".into(), status: ReportStatus::Done, blocked_on: None },
    );
    assert_eq!(rejection(r), "forbidden");
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

#[test]
fn items_get_thread_sequence_numbers_and_large_fields_go_to_blobs() {
    let db = db();
    started_task(&db);
    let t = register(&db).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = BlobStore::new(dir.path());

    let small = json!({ "text": "好的" });
    let mut large = json!({ "command": "cargo build", "output": "x".repeat(STORAGE_THRESHOLD + 1) });
    let blobs = externalize(&store, &mut large);
    assert_eq!(blobs.len(), 1);
    assert_eq!(large["output"]["size"], STORAGE_THRESHOLD + 1);
    assert_eq!(store.read(&blobs[0].hash).unwrap().len(), STORAGE_THRESHOLD + 1);

    let stored = db
        .write(|tx| {
            let item = |native, kind, content| NewItem {
                role: ROLE,
                turn_id: &t.turn_id,
                native_item_id: Some(native),
                kind,
                content,
                command_id: None,
            };
            let first = record_item(tx, &item("item_0", "agent_message", &small), &[], 1)?;
            let second = record_item(tx, &item("item_1", "command", &large), &blobs, 2)?;
            let replay = record_item(tx, &item("item_0", "agent_message", &small), &[], 3)?;
            Ok((first, second, replay))
        })
        .unwrap();
    assert_eq!(stored.0.unwrap().seq, 1);
    assert_eq!(stored.1.unwrap().seq, 2);
    assert_eq!(stored.2, None, "a replayed item is ignored");
}
