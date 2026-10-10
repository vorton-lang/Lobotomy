//! Shared setup for the command tests. Commit ids are opaque to the core; the store is not
//! involved here.
#![allow(dead_code)]

use lobotomy_core::capture::{CaptureResult, FinishCapture, pending_captures};
use lobotomy_core::project::Onboard;
use lobotomy_core::report::{OrgReport, ReportEffect, ReportStatus};
use lobotomy_core::task::{AttemptStarted, CreateTask, SendMessage, StartAttempt};
use lobotomy_core::turn::{EndTurn, Outcome, RegisterTurn, SessionIdentified, TurnRegistered};
use lobotomy_core::verify::{CheckOutcome, FinishVerification, StartVerification};
use lobotomy_core::workspace::{AlignIdleSlot, WorkspaceReady, WorkspaceState, current_workspace};
use lobotomy_core::{Caller, Db};

pub const ROLE: &str = "Malkuth";
pub const SLOT: &str = "worker";
/// The integration version at onboarding.
pub const BASE: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0";

/// A database with a connected repository.
pub fn db() -> Db {
    onboard(Db::open_in_memory().unwrap())
}

/// Connects a repository to the database, as the user does first.
pub fn onboard(db: Db) -> Db {
    db.execute(
        &Caller::User,
        &Onboard {
            request_id: "onboard".into(),
            repo_path: "C:/repo".into(),
            branch: "main".into(),
            head: BASE.into(),
            author_name: "Test User".into(),
            author_email: "user@example.com".into(),
        },
    )
    .unwrap();
    db
}

pub fn rejection<T: std::fmt::Debug>(r: lobotomy_core::Result<T>) -> &'static str {
    r.unwrap_err().code().expect("expected a rejection")
}

/// What the runtime does after the store wrote the slot.
pub fn ready_slot(db: &Db) {
    let ws = db.read(|c| current_workspace(c, SLOT)).unwrap().expect("the slot was planned");
    if ws.state == WorkspaceState::Materializing {
        db.execute(&Caller::Runtime, &WorkspaceReady { workspace_id: ws.id, target: ws.target, head: ws.head })
            .unwrap();
    }
}

/// Starts the task's next attempt and materializes the slot.
pub fn start(db: &Db, task: &str) -> AttemptStarted {
    let started = db.execute(&Caller::Runtime, &StartAttempt { task_id: task.into(), code_start: None }).unwrap();
    ready_slot(db);
    started
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
        trial: None,
    };
    db.execute(&Caller::Role { role: ROLE.into(), turn_id: turn_id.into() }, &report)
}

/// The turn ends normally; its capture stays pending.
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
    };
    db.execute(&Caller::User, &message).unwrap().id
}

/// The idle role's slot at the integration version, as the scheduler prepares it.
pub fn idle_slot(db: &Db) {
    db.execute(&Caller::Runtime, &AlignIdleSlot { role: ROLE.into() }).unwrap();
    ready_slot(db);
}

/// What the store would report for every pending capture: pinned, with a made-up commit.
pub fn pin_captures(db: &Db) -> Vec<String> {
    let mut commits = Vec::new();
    for capture in db.read(pending_captures).unwrap() {
        let commit = format!("commit-of-{}", capture.id);
        let result = CaptureResult::Pinned { commit: commit.clone(), changed: vec![] };
        db.execute(&Caller::Runtime, &FinishCapture { capture_id: capture.id, result }).unwrap();
        commits.push(commit);
    }
    commits
}

/// A task whose executor reported done; its capture is pinned as the candidate.
pub fn candidate(db: &Db) -> (String, String) {
    let task = create(db, "c1");
    start(db, &task);
    let t = register(db);
    report(db, &t.turn_id, ReportStatus::Done, None).unwrap();
    end_turn(db, &t.turn_id);
    let commit = pin_captures(db).pop().unwrap();
    (task, commit)
}

/// Verifies the task's candidate and returns the verification id.
pub fn verify(db: &Db, task: &str, commit: &str, conflicts: Vec<String>, checks: Vec<CheckOutcome>) -> String {
    let v = db.execute(&Caller::Runtime, &StartVerification { task_id: task.into() }).unwrap();
    let finish =
        FinishVerification { verification_id: v.clone(), commit: commit.into(), conflicts, checks, blobs: vec![] };
    db.execute(&Caller::Runtime, &finish).unwrap();
    v
}
