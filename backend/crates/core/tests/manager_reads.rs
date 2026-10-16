//! What the manager reads about a task (data-model.md §10.6 "读取带边界"): the read says up to which
//! event it is.

mod common;

use common::*;
use lobotomy_core::Caller;
use lobotomy_core::report::{ReportEffect, ReportStatus};
use lobotomy_core::task::SendBack;
use lobotomy_core::view::last_event;
use lobotomy_core::view::manager::task_read;

#[test]
fn a_task_read_lists_every_attempt_and_report() {
    let db = db();
    let task = candidate(&db);
    let read = db.read(|c| task_read(c, &task)).unwrap();
    assert_eq!(read.as_of, db.read(last_event).unwrap());
    assert_eq!((read.body.as_str(), read.criteria.as_str()), ("原话", "文件存在"));

    // Sent back and done again: both attempts and both reports are listed.
    let back = SendBack { request_id: "b1".into(), task_id: task.clone(), reason: "改名".into() };
    db.execute(&Caller::User, &back).unwrap();
    let t = register(&db);
    report(&db, &t.turn_id, ReportStatus::Done, None).unwrap();
    end_turn(&db, &t.turn_id);
    let read = db.read(|c| task_read(c, &task)).unwrap();
    assert_eq!(read.attempts.len(), 2);
    let effects: Vec<_> = read.reports.iter().map(|r| r.effect).collect();
    assert_eq!(effects, [Some(ReportEffect::Recorded), Some(ReportEffect::Recorded)]);
    assert_eq!(read.reports[0].report["status"], "done");
}
