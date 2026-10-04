//! Shared setup for the command tests. Commit ids are opaque to the core; the store is not
//! involved here.
#![allow(dead_code)]

use lobotomy_core::capture::{CaptureResult, FinishCapture, pending_captures};
use lobotomy_core::project::Onboard;
use lobotomy_core::task::{AttemptStarted, StartAttempt};
use lobotomy_core::workspace::{WorkspaceReady, WorkspaceState, current_workspace};
use lobotomy_core::{Caller, Db};

pub const ROLE: &str = "Malkuth";
pub const SLOT: &str = "worker";
/// The integration version at onboarding.
pub const BASE: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0";

/// A database with a connected repository.
pub fn db() -> Db {
    let db = Db::open_in_memory().unwrap();
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
        db.execute(&Caller::Runtime, &WorkspaceReady { workspace_id: ws.id, target: ws.target, head: ws.head }).unwrap();
    }
}

/// Starts the task's next attempt and materializes the slot.
pub fn start(db: &Db, task: &str) -> AttemptStarted {
    let started = db.execute(&Caller::Runtime, &StartAttempt { task_id: task.into(), code_start: None }).unwrap();
    ready_slot(db);
    started
}

/// What the store would report for every pending capture: pinned, with a made-up commit.
pub fn pin_captures(db: &Db) -> Vec<String> {
    let mut commits = Vec::new();
    for capture in db.read(pending_captures).unwrap() {
        let commit = format!("commit-of-{}", capture.id);
        let result = CaptureResult::Pinned { commit: commit.clone() };
        db.execute(&Caller::Runtime, &FinishCapture { capture_id: capture.id, result }).unwrap();
        commits.push(commit);
    }
    commits
}
