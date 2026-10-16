//! Acceptance and sending back (data-model.md §4, §9.2).

use lobotomy_core::task::{Accept, EditCriteria, Phase, SendBack, load_task, occupant, open_attempt, queued_messages};
use lobotomy_core::turn::current_session;
use lobotomy_core::{Caller, Db};

mod common;
use common::{ROLE, candidate, db, register, rejection};

fn phase(db: &Db, task: &str) -> Phase {
    db.read(|c| load_task(c, task)).unwrap().phase
}

fn accept(db: &Db, request_id: &str, task: &str) -> lobotomy_core::Result<()> {
    db.execute(
        &Caller::User,
        &Accept { request_id: request_id.into(), task_id: task.into(), criteria_version: 1, dropping: vec![] },
    )
}

#[test]
fn accepting_a_candidate_closes_the_task() {
    let db = db();
    let task = candidate(&db);
    assert_eq!(phase(&db, &task), Phase::Accepting);
    accept(&db, "a1", &task).unwrap();
    assert_eq!(phase(&db, &task), Phase::Done);
    assert_eq!(db.read(|c| occupant(c, ROLE)).unwrap(), None);
    assert!(db.read(|c| current_session(c, ROLE, Some(&task))).unwrap().is_none());
    // Accepting again with the same request changes nothing.
    accept(&db, "a1", &task).unwrap();
}

#[test]
fn accepting_checks_the_criteria() {
    let db = db();
    let task = candidate(&db);
    let edit =
        EditCriteria {
            request_id: "e1".into(), task_id: task.clone(), expected_version: 1, text: "更严的条件".into()
        };
    db.execute(&Caller::User, &edit).unwrap();
    assert_eq!(rejection(accept(&db, "a1", &task)), "stale_criteria");
    assert_eq!(phase(&db, &task), Phase::Accepting);
}

#[test]
fn sending_back_opens_the_next_attempt_with_the_reason() {
    let db = db();
    let task = candidate(&db);
    db.execute(
        &Caller::User,
        &SendBack { request_id: "b1".into(), task_id: task.clone(), reason: "文件名要改成 NEW.txt".into() },
    )
    .unwrap();
    assert_eq!(phase(&db, &task), Phase::Executing);
    assert_eq!(db.read(|c| open_attempt(c, &task)).unwrap().unwrap().seq, 2);
    let body = db.read(|c| queued_messages(c, ROLE)).unwrap().pop().unwrap().body;
    assert!(body.contains("NEW.txt"), "{body}");
    // The executor runs again in the same session.
    register(&db);
}
