//! Shared setup for the command tests.
#![allow(dead_code)]

use lobotomy_core::project::Onboard;
use lobotomy_core::report::{OrgReport, ReportEffect, ReportStatus};
use lobotomy_core::task::{AttemptStarted, CreateTask, SendMessage, StartAttempt};
use lobotomy_core::turn::{EndTurn, Outcome, RegisterTurn, SessionIdentified, TurnRegistered};
use lobotomy_core::{Caller, Db};

pub const ROLE: &str = "Malkuth";

/// A database with a connected repository.
pub fn db() -> Db {
    onboard(Db::open_in_memory().unwrap())
}

/// Connects a repository to the database, as the user does first.
pub fn onboard(db: Db) -> Db {
    db.execute(
        &Caller::User,
        &Onboard { request_id: "onboard".into(), project_id: "prj_test".into(), repo_path: "C:/repo".into() },
    )
    .unwrap();
    db
}

pub fn rejection<T: std::fmt::Debug>(r: lobotomy_core::Result<T>) -> &'static str {
    r.unwrap_err().code().expect("expected a rejection")
}

/// Starts the task's next attempt.
pub fn start(db: &Db, task: &str) -> AttemptStarted {
    db.execute(&Caller::Runtime, &StartAttempt { task_id: task.into() }).unwrap()
}

/// A task for the role, as the user creates it.
pub fn create(db: &Db, request_id: &str) -> String {
    create_titled(db, request_id, "加一个 new.txt")
}

pub fn create_titled(db: &Db, request_id: &str, title: &str) -> String {
    let task = CreateTask {
        request_id: request_id.into(),
        title: title.into(),
        body: "原话".into(),
        criteria: "文件存在".into(),
        executor: ROLE.into(),
    };
    db.execute(&Caller::User, &task).unwrap().id
}

/// Registers the role's next turn and identifies its harness session, as `thread.started` does.
pub fn register(db: &Db) -> TurnRegistered {
    let t = db.execute(&Caller::Runtime, &RegisterTurn { role: ROLE.into() }).unwrap();
    db.execute(&Caller::Runtime, &SessionIdentified { turn_id: t.turn_id.clone(), native_id: "thread-1".into() })
        .unwrap();
    t
}

/// The role's report from its turn.
pub fn report(
    db: &Db,
    turn_id: &str,
    status: ReportStatus,
    blocked_on: Option<&str>,
) -> lobotomy_core::Result<ReportEffect> {
    let report = OrgReport {
        title: "完成了".into(),
        body: "加了 new.txt".into(),
        status,
        blocked_on: blocked_on.map(Into::into),
    };
    db.execute(&Caller::Role { role: ROLE.into(), turn_id: turn_id.into() }, &report)
}

/// The turn ends normally.
pub fn end_turn(db: &Db, turn_id: &str) {
    db.execute(&Caller::Runtime, &EndTurn { turn_id: turn_id.into(), outcome: Outcome::Completed, failure: None })
        .unwrap();
}

/// The user's message to the role, for a task or outside any.
pub fn send(db: &Db, request_id: &str, task: Option<&str>, body: &str) -> String {
    let message = SendMessage {
        request_id: request_id.into(),
        role: ROLE.into(),
        task_id: task.map(Into::into),
        body: body.into(),
        reply_to: None,
        stop_id: None,
    };
    db.execute(&Caller::User, &message).unwrap().id
}

/// A task whose executor reported done in a turn that has ended: it waits for acceptance.
pub fn candidate(db: &Db) -> String {
    let task = create(db, "c1");
    start(db, &task);
    let t = register(db);
    report(db, &t.turn_id, ReportStatus::Done, None).unwrap();
    end_turn(db, &t.turn_id);
    task
}
