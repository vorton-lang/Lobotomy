//! Each event names the command that wrote it, and through its record who issued it
//! (data-model.md §7.6, §10.6).

use lobotomy_core::report::ReportStatus;
use lobotomy_core::task::{SetPaused, StartAttempt};
use lobotomy_core::view::{Event, events_after, last_event};
use lobotomy_core::{Caller, Db};

mod common;
use common::{create, db, ready_slot, register, report};

fn events(db: &Db, after: i64) -> Vec<Event> {
    db.read(|c| events_after(c, after)).unwrap()
}

fn last(db: &Db) -> i64 {
    db.read(last_event).unwrap()
}

fn recorded(db: &Db, command_id: &str) -> bool {
    let n: i64 = db
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM command_record WHERE id = ?1", [command_id], |r| r.get(0))?))
        .unwrap();
    n == 1
}

#[test]
fn all_events_of_a_command_name_its_record() {
    let db = db();
    let task = create(&db, "r1");
    let before = last(&db);
    let (_, id) = db.execute_recorded(&Caller::Runtime, &StartAttempt { task_id: task, code_start: None }).unwrap();
    let written = events(&db, before);
    assert!(written.len() > 1, "starting an attempt writes several events");
    assert!(written.iter().all(|e| e.command_id.as_deref() == Some(id.as_str())), "every one names the command");
    assert!(recorded(&db, &id));
}

#[test]
fn events_tell_the_user_the_runtime_and_a_role_apart() {
    let db = db();
    let task = create(&db, "r1");
    db.execute(&Caller::Runtime, &StartAttempt { task_id: task, code_start: None }).unwrap();
    ready_slot(&db);
    let turn = register(&db);
    report(&db, &turn.turn_id, ReportStatus::Progress, None).unwrap();

    let all = events(&db, 0);
    assert!(all.iter().all(|e| e.command_id.as_deref().is_some_and(|id| recorded(&db, id))));
    let source = |kind: &str| {
        let e = all.iter().find(|e| e.kind == kind).unwrap_or_else(|| panic!("no {kind} event"));
        (e.caller.as_deref(), e.caller_turn_id.as_deref())
    };
    assert_eq!(source("task.created"), (Some("user"), None));
    assert_eq!(source("attempt.started"), (Some("runtime"), None));
    assert_eq!(source("turn.registered"), (Some("runtime"), None));
    assert_eq!(source("report.recorded"), (Some("role:Malkuth"), Some(turn.turn_id.as_str())));
}

#[test]
fn a_replayed_command_adds_no_events() {
    let db = db();
    let task = create(&db, "r1");
    let pause = SetPaused { request_id: "k".into(), task_id: task, paused: true };
    let (_, first) = db.execute_recorded(&Caller::User, &pause).unwrap();
    let after_first = last(&db);
    let (_, again) = db.execute_recorded(&Caller::User, &pause).unwrap();
    assert_eq!(again, first, "the replay returns the first record");
    assert!(events(&db, after_first).is_empty());
    let paused = events(&db, 0).into_iter().filter(|e| e.command_id.as_deref() == Some(first.as_str())).count();
    assert_eq!(paused, 1);
}
