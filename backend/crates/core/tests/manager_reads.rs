//! What the manager reads about a task (data-model.md §10.6 "读取带边界"): the read says up to which
//! event it is, and a result that no longer holds is marked, not left out.

mod common;

use common::*;
use lobotomy_core::Caller;
use lobotomy_core::project::{Check, EditProjectConfig, ProjectConfig};
use lobotomy_core::report::{ReportEffect, ReportStatus};
use lobotomy_core::verify::SendBack;
use lobotomy_core::view::last_event;
use lobotomy_core::view::manager::task_read;

#[test]
fn a_replaced_candidate_and_outdated_checks_are_marked_not_left_out() {
    let db = db();
    let (task, _) = candidate(&db);
    verify(&db, &task, "rebased", vec![], vec![]);
    let read = db.read(|c| task_read(c, &task)).unwrap();
    assert_eq!(read.as_of, db.read(last_event).unwrap());
    assert_eq!((read.body.as_str(), read.criteria.as_str()), ("原话", "文件存在"));
    let first = read.attempts[0].candidate.as_ref().unwrap();
    assert!(!first.superseded && !first.verifications[0].stale);

    // New checks: the verification that passed no longer counts (data-model.md §4.5).
    let config =
        ProjectConfig { checks: vec![Check { command: "cargo test".into(), timeout_secs: 60 }], ..Default::default() };
    db.execute(&Caller::User, &EditProjectConfig { request_id: "p1".into(), expected_version: 1, config }).unwrap();
    let read = db.read(|c| task_read(c, &task)).unwrap();
    assert!(read.attempts[0].candidate.as_ref().unwrap().verifications[0].stale);

    // Sent back and done again: the first candidate is replaced and still listed.
    let back = SendBack { request_id: "b1".into(), task_id: task.clone(), reason: "改名".into() };
    db.execute(&Caller::User, &back).unwrap();
    ready_slot(&db);
    let t = register(&db);
    report(&db, &t.turn_id, ReportStatus::Done, None).unwrap();
    end_turn(&db, &t.turn_id);
    pin_captures(&db);
    let read = db.read(|c| task_read(c, &task)).unwrap();
    let superseded: Vec<bool> = read.attempts.iter().map(|a| a.candidate.as_ref().unwrap().superseded).collect();
    assert_eq!(superseded, [true, false]);
    let effects: Vec<_> = read.reports.iter().map(|r| r.effect).collect();
    assert_eq!(effects, [Some(ReportEffect::Recorded), Some(ReportEffect::Recorded)]);
    assert_eq!(read.reports[0].report["status"], "done");
}
