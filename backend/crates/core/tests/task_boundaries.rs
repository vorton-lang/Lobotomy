//! What crosses a task's boundary (#14): messages the executor never got when the task closes, and
//! reports from a turn outside any task (data-model.md §4.2).

use lobotomy_core::report::{ReportEffect, ReportStatus};
use lobotomy_core::task::{Abandon, Accept, EditCriteria, SendBack, queued_messages};
use lobotomy_core::turn::RegisterTurn;
use lobotomy_core::{Caller, Db};

mod common;
use common::{ROLE, candidate, create, db, register, rejection, send};

fn report(db: &Db, turn_id: &str, status: ReportStatus) -> ReportEffect {
    common::report(db, turn_id, status, None).unwrap()
}

fn accept(db: &Db, request_id: &str, task: &str, dropping: Vec<String>) -> lobotomy_core::Result<()> {
    let accept = Accept { request_id: request_id.into(), task_id: task.into(), criteria_version: 1, dropping };
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
    let task = candidate(&db);
    let added = send(&db, "m1", Some(&task), "异常行要能看到原始行号");

    assert_eq!(rejection(accept(&db, "a1", &task, vec![])), "undelivered_messages");
    assert_eq!(rejection(accept(&db, "a2", &task, vec!["msg_other".into()])), "undelivered_messages");
    accept(&db, "a3", &task, vec![added.clone()]).unwrap();

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
    let task = candidate(&db);
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
    let task = candidate(&db);
    db.execute(
        &Caller::User,
        &EditCriteria { request_id: "e1".into(), task_id: task.clone(), expected_version: 1, text: "新条件".into() },
    )
    .unwrap();
    let accept_v2 = Accept { request_id: "a1".into(), task_id: task.clone(), criteria_version: 2, dropping: vec![] };
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
    send(&db, "m1", None, "改个错字");
    let t = register(&db);
    assert_eq!(report(&db, &t.turn_id, ReportStatus::Done), ReportEffect::NoTask);
    assert_eq!(report(&db, &t.turn_id, ReportStatus::Progress), ReportEffect::Recorded);
}
