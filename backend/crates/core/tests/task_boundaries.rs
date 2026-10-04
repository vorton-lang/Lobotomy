//! What crosses a task's boundary (#14): messages the executor never got when the task closes,
//! reports from a turn outside any task, and the changes such a turn leaves in the slot
//! (data-model.md §4.2; harness-adapter.md §3).

use lobotomy_core::capture::{
    CaptureResult, DiscardOutsideChanges, FinishCapture, Outside, outside_changes, pending_captures,
};
use lobotomy_core::report::{OrgReport, ReportEffect, ReportStatus};
use lobotomy_core::task::{
    Abandon, AdoptOutsideChanges, CodeStart, CreateTask, EditCriteria, SendMessage, StartAttempt, load_task,
    queued_messages,
};
use lobotomy_core::turn::{EndTurn, Outcome, RegisterTurn, SessionIdentified, TurnRegistered};
use lobotomy_core::verify::{Accept, FinishVerification, SendBack, StartVerification, work_so_far};
use lobotomy_core::workspace::{AlignIdleSlot, WorkspaceState, current_workspace};
use lobotomy_core::{Caller, Db};

mod common;
use common::{BASE, ROLE, SLOT, db, pin_captures, ready_slot, rejection, start};

/// The idle role's slot at the integration version, as the scheduler prepares it.
fn idle_slot(db: &Db) {
    db.execute(&Caller::Runtime, &AlignIdleSlot { role: ROLE.into() }).unwrap();
    ready_slot(db);
}

fn create(db: &Db, request_id: &str) -> String {
    let task = CreateTask {
        request_id: request_id.into(),
        title: "加一个 new.txt".into(),
        body: "原话".into(),
        criteria: "文件存在".into(),
        executor: ROLE.into(),
    };
    db.execute(&Caller::User, &task).unwrap().id
}

fn register(db: &Db) -> TurnRegistered {
    let t = db.execute(&Caller::Runtime, &RegisterTurn { role: ROLE.into() }).unwrap();
    db.execute(&Caller::Runtime, &SessionIdentified { turn_id: t.turn_id.clone(), native_id: "thread".into() })
        .unwrap();
    t
}

fn report(db: &Db, turn_id: &str, status: ReportStatus) -> ReportEffect {
    let report = OrgReport { title: "完成了".into(), body: "加了 new.txt".into(), status, blocked_on: None };
    db.execute(&Caller::Role { role: ROLE.into(), turn_id: turn_id.into() }, &report).unwrap()
}

fn end_turn(db: &Db, turn_id: &str) {
    db.execute(&Caller::Runtime, &EndTurn { turn_id: turn_id.into(), outcome: Outcome::Completed, failure: None })
        .unwrap();
}

fn send(db: &Db, request_id: &str, task: Option<&str>, body: &str) -> String {
    let message = SendMessage {
        request_id: request_id.into(),
        role: ROLE.into(),
        task_id: task.map(Into::into),
        body: body.into(),
    };
    db.execute(&Caller::User, &message).unwrap().id
}

/// A task waiting for acceptance, and its passed verification.
fn accepting(db: &Db) -> (String, String) {
    let task = create(db, "c1");
    start(db, &task);
    let t = register(db);
    report(db, &t.turn_id, ReportStatus::Done);
    end_turn(db, &t.turn_id);
    let commit = pin_captures(db).pop().unwrap();
    let v = db.execute(&Caller::Runtime, &StartVerification { task_id: task.clone() }).unwrap();
    let finish =
        FinishVerification { verification_id: v.clone(), commit, conflicts: vec![], checks: vec![], blobs: vec![] };
    db.execute(&Caller::Runtime, &finish).unwrap();
    (task, v)
}

fn accept(db: &Db, request_id: &str, task: &str, v: &str, dropping: Vec<String>) -> lobotomy_core::Result<String> {
    let accept = Accept {
        request_id: request_id.into(),
        task_id: task.into(),
        verification_id: v.into(),
        criteria_version: 1,
        expected_integration: BASE.into(),
        dropping,
    };
    db.execute(&Caller::User, &accept)
}

fn message_state(db: &Db, id: &str) -> String {
    db.read(|c| Ok(c.query_row("SELECT state FROM message WHERE id = ?1", [id], |r| r.get(0))?)).unwrap()
}

/// The scenario of #14: the user adds to the task after the executor reported done. Accepting
/// does not let the addition slip away: the user lets it go explicitly, and it is never delivered.
#[test]
fn accepting_lets_go_of_undelivered_messages_only_when_told() {
    let db = db();
    let (task, v) = accepting(&db);
    let added = send(&db, "m1", Some(&task), "异常行要能看到原始行号");

    assert_eq!(rejection(accept(&db, "a1", &task, &v, vec![])), "undelivered_messages");
    assert_eq!(rejection(accept(&db, "a2", &task, &v, vec!["msg_other".into()])), "undelivered_messages");
    accept(&db, "a3", &task, &v, vec![added.clone()]).unwrap();

    assert_eq!(message_state(&db, &added), "dropped");
    assert!(db.read(|c| queued_messages(c, ROLE)).unwrap().is_empty(), "nothing waits for a closed task");
    let detail: String =
        db.read(|c| Ok(c.query_row("SELECT detail FROM decision WHERE kind = 'accept'", [], |r| r.get(0))?)).unwrap();
    assert!(detail.contains("异常行要能看到原始行号"), "the decision records what was let go: {detail}");
    // Nothing is left for a turn outside the task.
    assert_eq!(rejection(db.execute(&Caller::Runtime, &RegisterTurn { role: ROLE.into() })), "nothing_to_deliver");
}

/// Sending the task back delivers the addition with the reason, in the task's next attempt.
#[test]
fn sending_back_delivers_the_undelivered_messages_in_the_task() {
    let db = db();
    let (task, _) = accepting(&db);
    send(&db, "m1", Some(&task), "异常行要能看到原始行号");
    db.execute(
        &Caller::User,
        &SendBack { request_id: "b1".into(), task_id: task.clone(), reason: "按补充要求改".into() },
    )
    .unwrap();
    let t = register(&db);
    let input: (String, Option<String>) = db
        .read(|c| {
            Ok(c.query_row("SELECT input, task_id FROM turn WHERE id = ?1", [&t.turn_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?)
        })
        .unwrap();
    assert!(input.0.contains("异常行要能看到原始行号") && input.0.contains("按补充要求改"), "{}", input.0);
    assert_eq!(input.1.as_deref(), Some(task.as_str()));
}

/// The runtime's own messages to a task, such as a criteria update, need no decision and go with
/// the task; abandoning drops the user's too.
#[test]
fn closing_a_task_drops_its_queued_messages() {
    let db = db();
    let (task, v) = accepting(&db);
    db.execute(
        &Caller::User,
        &EditCriteria { request_id: "e1".into(), task_id: task.clone(), expected_version: 1, text: "新条件".into() },
    )
    .unwrap();
    let accept_v2 = Accept {
        request_id: "a1".into(),
        task_id: task.clone(),
        verification_id: v,
        criteria_version: 2,
        expected_integration: BASE.into(),
        dropping: vec![],
    };
    db.execute(&Caller::User, &accept_v2).unwrap();
    assert!(db.read(|c| queued_messages(c, ROLE)).unwrap().is_empty());

    let other = create(&db, "c2");
    let added = send(&db, "m2", Some(&other), "补充");
    db.execute(&Caller::User, &Abandon { request_id: "x1".into(), task_id: other.clone(), reason: "不做了".into() })
        .unwrap();
    assert_eq!(message_state(&db, &added), "dropped");
}

/// A turn outside any task has nothing for done to advance, and is told so, apart from a late
/// report of a task's turn.
#[test]
fn a_report_from_a_turn_outside_any_task_has_no_task() {
    let db = db();
    idle_slot(&db);
    send(&db, "m1", None, "改个错字");
    let t = register(&db);
    assert_eq!(report(&db, &t.turn_id, ReportStatus::Done), ReportEffect::NoTask);
    assert_eq!(report(&db, &t.turn_id, ReportStatus::Progress), ReportEffect::Recorded);
}

/// Runs a turn outside any task that changes `paths`, and returns its capture.
fn outside_turn(db: &Db, request_id: &str, paths: &[&str]) -> String {
    send(db, request_id, None, "改个错字");
    let t = register(db);
    end_turn(db, &t.turn_id);
    let capture = db.read(pending_captures).unwrap().pop().unwrap();
    let changed = paths.iter().map(|p| (*p).to_owned()).collect();
    let result = CaptureResult::Pinned { commit: format!("outside-{request_id}"), changed };
    db.execute(&Caller::Runtime, &FinishCapture { capture_id: capture.id.clone(), result }).unwrap();
    capture.id
}

#[test]
fn changes_outside_any_task_keep_tasks_from_starting_until_the_user_decides() {
    let db = db();
    idle_slot(&db);
    // A turn that changed nothing leaves nothing to decide.
    outside_turn(&db, "m0", &[]);
    assert_eq!(db.read(|c| outside_changes(c, ROLE)).unwrap(), None);

    let capture = outside_turn(&db, "m1", &["README.md"]);
    let pending = db.read(|c| outside_changes(c, ROLE)).unwrap().unwrap();
    assert_eq!((pending.id.as_str(), pending.outside), (capture.as_str(), Some(Outside::Pending)));
    let task = create(&db, "c1");
    assert_eq!(
        rejection(db.execute(&Caller::Runtime, &StartAttempt { task_id: task.clone(), code_start: None })),
        "outside_changes"
    );

    // Discarding writes the slot over with the integration version; then the task can start.
    db.execute(&Caller::User, &DiscardOutsideChanges { request_id: "d1".into(), capture_id: capture.clone() }).unwrap();
    assert_eq!(db.read(|c| outside_changes(c, ROLE)).unwrap(), None);
    let ws = db.read(|c| current_workspace(c, SLOT)).unwrap().unwrap();
    assert_eq!((ws.state, ws.target.as_str(), ws.task_id), (WorkspaceState::Materializing, BASE, None));
    ready_slot(&db);
    start(&db, &task);
}

/// When the integration version moves (another role's task was accepted), the idle slot follows
/// it, but not over changes the user has not decided about (#16).
#[test]
fn an_idle_slot_does_not_follow_the_integration_version_over_undecided_changes() {
    let db = db();
    idle_slot(&db);
    outside_turn(&db, "m1", &["README.md"]);
    db.write(|tx| {
        Ok(tx.execute("UPDATE project SET integration = 'moved', integration_rev = integration_rev + 1", [])?)
    })
    .unwrap();
    assert_eq!(rejection(db.execute(&Caller::Runtime, &AlignIdleSlot { role: ROLE.into() })), "outside_changes");
    assert_eq!(db.read(|c| current_workspace(c, SLOT)).unwrap().unwrap().target, BASE);
}

#[test]
fn a_task_made_from_outside_changes_starts_from_them() {
    let db = db();
    idle_slot(&db);
    let capture = outside_turn(&db, "m1", &["README.md"]);
    let adopt = AdoptOutsideChanges {
        request_id: "t1".into(),
        capture_id: capture.clone(),
        title: "改错字".into(),
        body: "把刚才的改动做完".into(),
        criteria: "-".into(),
    };
    let task = db.execute(&Caller::User, &adopt).unwrap().id;
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().origin_capture.as_deref(), Some(capture.as_str()));
    assert_eq!(db.read(|c| outside_changes(c, ROLE)).unwrap(), None);
    // The same capture cannot be settled twice.
    let again = DiscardOutsideChanges { request_id: "d1".into(), capture_id: capture.clone() };
    assert_eq!(rejection(db.execute(&Caller::User, &again)), "not_pending");

    // Like a reopened task, its first attempt starts from its work so far, rebased by the runtime.
    assert_eq!(db.read(|c| work_so_far(c, &task)).unwrap().as_deref(), Some("outside-m1"));
    let plain = StartAttempt { task_id: task.clone(), code_start: None };
    assert_eq!(rejection(db.execute(&Caller::Runtime, &plain)), "code_start_needed");
    let code_start = CodeStart { commit: "outside-m1-rebased".into(), base: BASE.into(), conflicts: vec![] };
    db.execute(&Caller::Runtime, &StartAttempt { task_id: task.clone(), code_start: Some(code_start) }).unwrap();
    let ws = db.read(|c| current_workspace(c, SLOT)).unwrap().unwrap();
    assert_eq!((ws.target.as_str(), ws.head.as_str()), ("outside-m1-rebased", BASE));
    let brief = db.read(|c| queued_messages(c, ROLE)).unwrap().pop().unwrap().body;
    assert!(brief.contains("初始改动"), "{brief}");
}
