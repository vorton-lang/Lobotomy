//! Captures, verification, acceptance and the preview (harness-adapter.md §4; data-model.md §4,
//! §5, §9.2).

use lobotomy_core::capture::{
    ApproveNewFiles, CaptureResult, CaptureState, DiscardUncaptured, FinishCapture, NewFile, UncoveredPath,
    latest_capture, pending_captures,
};
use lobotomy_core::project::{Check, EditProjectConfig, ProjectConfig, load_project};
use lobotomy_core::report::{OrgReport, ReportStatus};
use lobotomy_core::task::{
    Abandon, CodeStart, CreateTask, Phase, Reopen, SendMessage, StartAttempt, load_task, occupant, open_attempt,
    queued_messages,
};
use lobotomy_core::turn::{
    Continue, EndTurn, Outcome, RegisterTurn, SessionIdentified, TurnRegistered, current_session,
};
use lobotomy_core::verify::{
    Accept, CheckOutcome, FinishPreview, FinishVerification, PreviewResult, RetryPreview, Reverify, SendBack,
    StartVerification, VerificationState, latest_verification, next_preview, preview_stopped,
};
use lobotomy_core::workspace::{AlignIdleSlot, WorkspaceState, current_workspace};
use lobotomy_core::{Caller, Db};
use serde_json::json;

mod common;
use common::{BASE, ROLE, SLOT, db, pin_captures, ready_slot, rejection, start};

fn create(db: &Db, request_id: &str) -> String {
    db.execute(
        &Caller::User,
        &CreateTask {
            request_id: request_id.into(),
            title: "加一个 new.txt".into(),
            body: "原话".into(),
            criteria: "文件存在".into(),
            executor: ROLE.into(),
        },
    )
    .unwrap()
    .id
}

fn register(db: &Db) -> TurnRegistered {
    let t = db.execute(&Caller::Runtime, &RegisterTurn { role: ROLE.into() }).unwrap();
    db.execute(&Caller::Runtime, &SessionIdentified { turn_id: t.turn_id.clone(), native_id: "thread-1".into() })
        .unwrap();
    t
}

fn report_done(db: &Db, turn_id: &str) {
    db.execute(
        &Caller::Role { role: ROLE.into(), turn_id: turn_id.into() },
        &OrgReport {
            title: "完成了".into(),
            body: "加了 new.txt".into(),
            status: ReportStatus::Done,
            blocked_on: None,
        },
    )
    .unwrap();
}

fn end_turn(db: &Db, turn_id: &str) {
    db.execute(&Caller::Runtime, &EndTurn { turn_id: turn_id.into(), outcome: Outcome::Completed, failure: None })
        .unwrap();
}

fn pending(db: &Db) -> String {
    db.read(pending_captures).unwrap().pop().expect("a pending capture").id
}

fn finish_capture(db: &Db, capture_id: &str, result: CaptureResult) {
    db.execute(&Caller::Runtime, &FinishCapture { capture_id: capture_id.into(), result }).unwrap();
}

/// A task whose executor reported done; its capture is pinned as the candidate.
fn candidate(db: &Db) -> (String, String) {
    let task = create(db, "c1");
    start(db, &task);
    let t = register(db);
    report_done(db, &t.turn_id);
    end_turn(db, &t.turn_id);
    let commit = pin_captures(db).pop().unwrap();
    (task, commit)
}

fn check(exit_code: i64) -> CheckOutcome {
    CheckOutcome {
        command: "cargo test".into(),
        exit_code: Some(exit_code),
        timed_out: false,
        output: json!({ "text": "output" }),
        duration_ms: 5,
        tail: "test result: FAILED".into(),
    }
}

/// Verifies the task's candidate and returns the verification id.
fn verify(db: &Db, task: &str, commit: &str, conflicts: Vec<String>, checks: Vec<CheckOutcome>) -> String {
    let v = db.execute(&Caller::Runtime, &StartVerification { task_id: task.into() }).unwrap();
    let finish =
        FinishVerification { verification_id: v.clone(), commit: commit.into(), conflicts, checks, blobs: vec![] };
    db.execute(&Caller::Runtime, &finish).unwrap();
    v
}

fn phase(db: &Db, task: &str) -> Phase {
    db.read(|c| load_task(c, task)).unwrap().phase
}

fn accept(db: &Db, request_id: &str, task: &str, verification: &str, expected: &str) -> lobotomy_core::Result<String> {
    db.execute(
        &Caller::User,
        &Accept {
            request_id: request_id.into(),
            task_id: task.into(),
            verification_id: verification.into(),
            criteria_version: 1,
            expected_integration: expected.into(),
            dropping: vec![],
        },
    )
}

#[test]
fn a_verified_candidate_is_published_by_accepting_it() {
    let db = db();
    let (task, _) = candidate(&db);
    assert_eq!(phase(&db, &task), Phase::Verifying);
    let v = verify(&db, &task, "rebased", vec![], vec![check(0)]);
    assert_eq!(phase(&db, &task), Phase::Accepting);

    let published = accept(&db, "a1", &task, &v, BASE).unwrap();
    assert_eq!(published, "rebased");
    let project = db.read(load_project).unwrap().unwrap();
    assert_eq!((project.integration.as_str(), project.integration_rev), ("rebased", 2));
    assert_eq!(phase(&db, &task), Phase::Done);
    assert_eq!(db.read(|c| occupant(c, ROLE)).unwrap(), None);
    assert!(db.read(|c| current_session(c, ROLE, Some(&task))).unwrap().is_none());
    // The preview waits in the outbox, from the version the user's repository is at.
    let job = db.read(next_preview).unwrap().unwrap();
    assert_eq!((job.target.as_str(), job.previewed.as_str()), ("rebased", BASE));
    // Accepting again with the same request returns the same result.
    assert_eq!(accept(&db, "a1", &task, &v, BASE).unwrap(), "rebased");
}

#[test]
fn the_commit_message_names_the_task_the_report_and_the_role() {
    let db = db();
    let (task, _) = candidate(&db);
    let v = db.execute(&Caller::Runtime, &StartVerification { task_id: task.clone() }).unwrap();
    let plan = db.read(|c| lobotomy_core::verify::verification_plan(c, &v)).unwrap();
    assert!(plan.message.starts_with("加一个 new.txt\n\n完成了\n\n加了 new.txt\n\n"), "{}", plan.message);
    assert!(plan.message.ends_with(&format!("Lobotomy-Task: {task}\nLobotomy-Role: Malkuth\n")), "{}", plan.message);
    assert_eq!((plan.author_name.as_str(), plan.base.as_str()), ("Test User", BASE));
}

#[test]
fn accepting_checks_the_head_the_criteria_and_the_verification() {
    let db = db();
    let (task, _) = candidate(&db);
    let v = verify(&db, &task, "rebased", vec![], vec![]);
    assert_eq!(rejection(accept(&db, "a1", &task, &v, "elsewhere")), "integration_moved");
    assert_eq!(rejection(accept(&db, "a2", &task, "ver_other", BASE)), "stale_verification");
    let edit = lobotomy_core::task::EditCriteria {
        request_id: "e1".into(),
        task_id: task.clone(),
        expected_version: 1,
        text: "更严的条件".into(),
    };
    db.execute(&Caller::User, &edit).unwrap();
    assert_eq!(rejection(accept(&db, "a3", &task, &v, BASE)), "stale_criteria");
    // None of the rejections closed the task.
    assert_eq!(phase(&db, &task), Phase::Accepting);
}

#[test]
fn a_new_check_configuration_sends_the_task_back_to_verification() {
    let db = db();
    let (task, _) = candidate(&db);
    let v = verify(&db, &task, "rebased", vec![], vec![]);
    assert_eq!(rejection(db.execute(&Caller::Runtime, &Reverify { task_id: task.clone() })), "evidence_current");
    let config =
        ProjectConfig { checks: vec![Check { command: "cargo test".into(), timeout_secs: 60 }], ..Default::default() };
    db.execute(&Caller::User, &EditProjectConfig { request_id: "p1".into(), expected_version: 1, config }).unwrap();
    // The old checks do not cover the new configuration.
    assert_eq!(rejection(accept(&db, "a1", &task, &v, BASE)), "stale_verification");
    db.execute(&Caller::Runtime, &Reverify { task_id: task.clone() }).unwrap();
    assert_eq!(phase(&db, &task), Phase::Verifying);
    let again = db.execute(&Caller::Runtime, &StartVerification { task_id: task.clone() }).unwrap();
    let plan = db.read(|c| lobotomy_core::verify::verification_plan(c, &again)).unwrap();
    assert_eq!(plan.config.checks.len(), 1);
}

#[test]
fn a_failed_check_goes_straight_back_to_the_executor() {
    let db = db();
    let (task, _) = candidate(&db);
    verify(&db, &task, "rebased", vec![], vec![check(0), check(101)]);
    assert_eq!(phase(&db, &task), Phase::Executing);
    let attempt = db.read(|c| open_attempt(c, &task)).unwrap().unwrap();
    assert_eq!(attempt.seq, 2);
    let inbox = db.read(|c| queued_messages(c, ROLE)).unwrap();
    let body = &inbox.last().unwrap().body;
    assert!(
        body.contains("cargo test") && body.contains("退出码为 101") && body.contains("test result: FAILED"),
        "{body}"
    );
    // The candidate was made on the current integration version, so the slot already holds it.
    let ws = db.read(|c| current_workspace(c, SLOT)).unwrap().unwrap();
    assert_eq!(ws.state, WorkspaceState::Ready);
    // The executor runs again in the same session.
    register(&db);
}

#[test]
fn a_candidate_rebased_onto_a_newer_version_is_written_into_the_slot() {
    let db = db();
    let (task, _) = candidate(&db);
    // Another candidate was accepted meanwhile; the integration version moved.
    db.read(|c| Ok(c.execute("UPDATE project SET integration = 'newer'", [])?)).unwrap();
    verify(&db, &task, "merged", vec!["a.txt".into()], vec![]);
    assert_eq!(phase(&db, &task), Phase::Executing);
    let body = db.read(|c| queued_messages(c, ROLE)).unwrap().pop().unwrap().body;
    assert!(body.contains("冲突") && body.contains("a.txt"), "{body}");
    let ws = db.read(|c| current_workspace(c, SLOT)).unwrap().unwrap();
    assert_eq!((ws.state, ws.target.as_str(), ws.head.as_str()), (WorkspaceState::Materializing, "merged", "newer"));
    // No turn before the slot holds the merged result.
    assert_eq!(rejection(db.execute(&Caller::Runtime, &RegisterTurn { role: ROLE.into() })), "slot_not_ready");
    ready_slot(&db);
    register(&db);
}

#[test]
fn sending_back_opens_the_next_attempt_with_the_reason() {
    let db = db();
    let (task, _) = candidate(&db);
    verify(&db, &task, "rebased", vec![], vec![]);
    db.execute(
        &Caller::User,
        &SendBack { request_id: "b1".into(), task_id: task.clone(), reason: "文件名要改成 NEW.txt".into() },
    )
    .unwrap();
    assert_eq!(phase(&db, &task), Phase::Executing);
    assert_eq!(db.read(|c| open_attempt(c, &task)).unwrap().unwrap().seq, 2);
    let body = db.read(|c| queued_messages(c, ROLE)).unwrap().pop().unwrap().body;
    assert!(body.contains("NEW.txt"), "{body}");
}

#[test]
fn a_late_verification_advances_nothing() {
    let db = db();
    let (task, commit) = candidate(&db);
    let v = db.execute(&Caller::Runtime, &StartVerification { task_id: task.clone() }).unwrap();
    db.execute(&Caller::User, &Abandon { request_id: "x1".into(), task_id: task.clone(), reason: String::new() })
        .unwrap();
    let finish =
        FinishVerification { verification_id: v.clone(), commit, conflicts: vec![], checks: vec![], blobs: vec![] };
    assert_eq!(db.execute(&Caller::Runtime, &finish).unwrap(), VerificationState::Passed);
    assert_eq!(phase(&db, &task), Phase::Abandoned);
    assert_eq!(db.read(|c| latest_verification(c, &task)).unwrap().unwrap().id, v);
}

/// A done turn whose capture went over the guardrail.
fn stopped_done(db: &Db) -> (String, String) {
    let task = create(db, "c1");
    start(db, &task);
    let t = register(db);
    report_done(db, &t.turn_id);
    end_turn(db, &t.turn_id);
    let capture = pending(db);
    let files = vec![NewFile { path: "venv/lib.py".into(), size: 10 }, NewFile { path: "venv/x.py".into(), size: 20 }];
    finish_capture(db, &capture, CaptureResult::Oversized { files, total_bytes: 30 });
    (task, capture)
}

#[test]
fn a_stopped_capture_stops_the_role_until_the_user_decides() {
    let db = db();
    let (task, capture) = stopped_done(&db);
    // Nothing goes back to the executor on its own, and no turn starts, whatever arrives.
    assert!(db.read(|c| queued_messages(c, ROLE)).unwrap().is_empty());
    db.execute(
        &Caller::User,
        &SendMessage { request_id: "m1".into(), role: ROLE.into(), task_id: None, body: "在吗".into() },
    )
    .unwrap();
    assert_eq!(rejection(db.execute(&Caller::Runtime, &RegisterTurn { role: ROLE.into() })), "capture_stopped");
    assert_eq!(phase(&db, &task), Phase::Executing);

    // The user keeps the files: the slot is captured again without the guardrail, and the done
    // counts.
    db.execute(&Caller::User, &ApproveNewFiles { request_id: "k1".into(), capture_id: capture.clone() }).unwrap();
    let retried = db.read(|c| latest_capture(c, ROLE)).unwrap().unwrap();
    assert_eq!((retried.state, retried.options.ignore_guard), (CaptureState::Intent, true));
    finish_capture(&db, &capture, CaptureResult::Pinned { commit: "with-venv".into(), changed: vec![] });
    assert_eq!(phase(&db, &task), Phase::Verifying, "the approved capture is the candidate");
}

#[test]
fn continuing_past_a_stopped_done_hands_the_list_to_the_executor() {
    let db = db();
    let (task, _) = stopped_done(&db);
    let next = db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: ROLE.into() }).unwrap();
    let input = db.read(|c| lobotomy_core::turn::load_turn(c, &next.turn_id)).unwrap().input;
    assert!(input.contains("venv/lib.py") && input.contains("done 因此没有生效"), "{input}");
    assert_eq!(db.read(|c| open_attempt(c, &task)).unwrap().unwrap().done_turn_id, None, "the stopped done is void");

    // The executor cleans up and reports done again.
    report_done(&db, &next.turn_id);
    end_turn(&db, &next.turn_id);
    pin_captures(&db);
    assert_eq!(phase(&db, &task), Phase::Verifying);
}

#[test]
fn discarding_leaves_out_what_stopped_the_capture() {
    let db = db();
    let (task, capture) = stopped_done(&db);
    db.execute(&Caller::User, &DiscardUncaptured { request_id: "d1".into(), capture_id: capture.clone() }).unwrap();
    let retried = db.read(|c| latest_capture(c, ROLE)).unwrap().unwrap();
    assert!(retried.options.leave_new_files && !retried.options.ignore_guard);
    finish_capture(&db, &capture, CaptureResult::Pinned { commit: "without-venv".into(), changed: vec![] });
    assert_eq!(phase(&db, &task), Phase::Verifying);
}

#[test]
fn deciding_needs_the_latest_capture_and_no_running_turn() {
    let db = db();
    let task = create(&db, "c1");
    start(&db, &task);
    let t = register(&db);
    end_turn(&db, &t.turn_id);
    let first = pending(&db);
    finish_capture(&db, &first, CaptureResult::Oversized { files: vec![], total_bytes: 1 << 40 });
    let t2 = db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: ROLE.into() }).unwrap();
    let approve = ApproveNewFiles { request_id: "a1".into(), capture_id: first.clone() };
    assert_eq!(rejection(db.execute(&Caller::User, &approve)), "turn_unfinished");
    end_turn(&db, &t2.turn_id);
    pin_captures(&db);
    assert_eq!(rejection(db.execute(&Caller::User, &approve)), "not_latest");
    // The role runs normally again once its latest capture is pinned.
    db.execute(
        &Caller::User,
        &SendMessage { request_id: "m1".into(), role: ROLE.into(), task_id: None, body: "好".into() },
    )
    .unwrap();
    register(&db);
}

#[test]
fn a_slot_whose_last_capture_was_stopped_does_not_go_to_the_next_task() {
    let db = db();
    let a = create(&db, "c1");
    let b = create(&db, "c2");
    start(&db, &a);
    let t = register(&db);
    end_turn(&db, &t.turn_id);
    let capture = pending(&db);
    let paths = vec![UncoveredPath { kind: "nested_repo".into(), path: "vendor/lib".into() }];
    finish_capture(&db, &capture, CaptureResult::Uncovered { paths });
    db.execute(&Caller::User, &Abandon { request_id: "x1".into(), task_id: a, reason: String::new() }).unwrap();

    let start_b = StartAttempt { task_id: b.clone(), code_start: None };
    assert_eq!(rejection(db.execute(&Caller::Runtime, &start_b)), "slot_uncaptured");
    // The user decides to leave the nested repository out; the slot is captured again.
    db.execute(&Caller::User, &DiscardUncaptured { request_id: "d1".into(), capture_id: capture.clone() }).unwrap();
    assert_eq!(rejection(db.execute(&Caller::Runtime, &start_b)), "capture_pending");
    let retried = db.read(|c| latest_capture(c, ROLE)).unwrap().unwrap();
    assert!(retried.options.leave_uncovered);
    finish_capture(&db, &capture, CaptureResult::Pinned { commit: "without-vendor".into(), changed: vec![] });
    start(&db, &b);
}

#[test]
fn a_reopened_task_starts_from_its_last_candidate_rebased() {
    let db = db();
    let (task, _) = candidate(&db);
    db.execute(&Caller::User, &Abandon { request_id: "x1".into(), task_id: task.clone(), reason: String::new() })
        .unwrap();
    db.execute(&Caller::User, &Reopen { request_id: "o1".into(), task_id: task.clone() }).unwrap();
    let plain = StartAttempt { task_id: task.clone(), code_start: None };
    assert_eq!(rejection(db.execute(&Caller::Runtime, &plain)), "code_start_needed");
    let code_start =
        CodeStart { commit: "rebased-candidate".into(), base: BASE.into(), conflicts: vec!["a.txt".into()] };
    db.execute(&Caller::Runtime, &StartAttempt { task_id: task.clone(), code_start: Some(code_start) }).unwrap();
    let ws = db.read(|c| current_workspace(c, SLOT)).unwrap().unwrap();
    assert_eq!((ws.target.as_str(), ws.head.as_str()), ("rebased-candidate", BASE));
    let brief = db.read(|c| queued_messages(c, ROLE)).unwrap().pop().unwrap().body;
    assert!(brief.contains("冲突") && brief.contains("a.txt"), "{brief}");
}

fn align(db: &Db) -> lobotomy_core::Result<()> {
    db.execute(&Caller::Runtime, &AlignIdleSlot { role: ROLE.into() })
}

#[test]
fn a_role_without_a_task_gets_a_slot_at_the_integration_version() {
    let db = db();
    db.execute(
        &Caller::User,
        &SendMessage { request_id: "m1".into(), role: ROLE.into(), task_id: None, body: "在吗".into() },
    )
    .unwrap();
    assert_eq!(rejection(db.execute(&Caller::Runtime, &RegisterTurn { role: ROLE.into() })), "slot_not_ready");
    align(&db).unwrap();
    let ws = db.read(|c| current_workspace(c, SLOT)).unwrap().unwrap();
    assert_eq!((ws.target.as_str(), ws.head.as_str()), (BASE, BASE));
    ready_slot(&db);
    assert_eq!(rejection(align(&db)), "slot_aligned");
    register(&db);
}

#[test]
fn after_acceptance_the_idle_slot_follows_the_new_integration_version() {
    let db = db();
    let (task, _) = candidate(&db);
    // While the task is open the slot belongs to it.
    assert_eq!(rejection(align(&db)), "role_busy");
    let v = verify(&db, &task, "published", vec![], vec![]);
    accept(&db, "a1", &task, &v, BASE).unwrap();
    align(&db).unwrap();
    let ws = db.read(|c| current_workspace(c, SLOT)).unwrap().unwrap();
    assert_eq!(
        (ws.state, ws.target.as_str(), ws.head.as_str()),
        (WorkspaceState::Materializing, "published", "published")
    );
}

#[test]
fn after_abandoning_the_slot_is_reset_once_its_last_turn_is_captured() {
    let db = db();
    let task = create(&db, "c1");
    start(&db, &task);
    let t = register(&db);
    db.execute(&Caller::User, &Abandon { request_id: "x1".into(), task_id: task, reason: String::new() }).unwrap();
    assert_eq!(rejection(align(&db)), "turn_unfinished");
    end_turn(&db, &t.turn_id);
    assert_eq!(rejection(align(&db)), "capture_pending");
    pin_captures(&db);
    align(&db).unwrap();
    let ws = db.read(|c| current_workspace(c, SLOT)).unwrap().unwrap();
    assert_eq!((ws.target.as_str(), ws.head.as_str()), (BASE, BASE));
}

/// #12: B is accepted while A's preview is being written; A's receipt must not drop B's preview.
#[test]
fn an_older_preview_finishing_keeps_the_newer_one() {
    let db = db();
    let (a, _) = candidate(&db);
    let va = verify(&db, &a, "accepted-A", vec![], vec![]);
    accept(&db, "a1", &a, &va, BASE).unwrap();
    let in_flight = db.read(next_preview).unwrap().unwrap();

    let b = create(&db, "c2");
    start(&db, &b);
    let t = register(&db);
    report_done(&db, &t.turn_id);
    end_turn(&db, &t.turn_id);
    pin_captures(&db);
    let vb = verify(&db, &b, "accepted-B", vec![], vec![]);
    accept(&db, "a2", &b, &vb, "accepted-A").unwrap();
    db.execute(&Caller::Runtime, &FinishPreview { outbox_id: in_flight.outbox_id, result: PreviewResult::Written })
        .unwrap();

    assert_eq!(db.read(load_project).unwrap().unwrap().previewed, "accepted-A");
    let next = db.read(next_preview).unwrap().expect("B's preview still runs");
    assert_eq!((next.target.as_str(), next.previewed.as_str()), ("accepted-B", "accepted-A"));
    db.execute(&Caller::Runtime, &FinishPreview { outbox_id: next.outbox_id, result: PreviewResult::Written }).unwrap();
    assert_eq!(db.read(load_project).unwrap().unwrap().previewed, "accepted-B");
    assert_eq!(db.read(next_preview).unwrap(), None);
}

#[test]
fn a_verification_passes_only_when_every_check_ran() {
    let db = db();
    let config = ProjectConfig {
        checks: vec![
            Check { command: "cargo build".into(), timeout_secs: 60 },
            Check { command: "cargo test".into(), timeout_secs: 60 },
        ],
        ..Default::default()
    };
    db.execute(&Caller::User, &EditProjectConfig { request_id: "p1".into(), expected_version: 1, config }).unwrap();
    let (task, commit) = candidate(&db);
    let v = db.execute(&Caller::Runtime, &StartVerification { task_id: task.clone() }).unwrap();
    // Only the first check ran, for example because the task left verification meanwhile.
    let finish =
        FinishVerification { verification_id: v, commit, conflicts: vec![], checks: vec![check(0)], blobs: vec![] };
    db.execute(&Caller::User, &Abandon { request_id: "x1".into(), task_id: task.clone(), reason: String::new() })
        .unwrap();
    assert_eq!(db.execute(&Caller::Runtime, &finish).unwrap(), VerificationState::Failed);
}

#[test]
fn the_preview_is_written_once_for_the_latest_version_and_stops_on_divergence() {
    let db = db();
    let (task, _) = candidate(&db);
    let v = verify(&db, &task, "first", vec![], vec![]);
    accept(&db, "a1", &task, &v, BASE).unwrap();
    let job = db.read(next_preview).unwrap().unwrap();
    let stopped = PreviewResult::Stopped { reason: "工作区有改动：a.txt".into() };
    db.execute(&Caller::Runtime, &FinishPreview { outbox_id: job.outbox_id.clone(), result: stopped }).unwrap();
    assert_eq!(db.read(next_preview).unwrap(), None);
    assert_eq!(db.read(preview_stopped).unwrap().as_deref(), Some("工作区有改动：a.txt"));

    db.execute(&Caller::User, &RetryPreview { request_id: "r1".into() }).unwrap();
    let job = db.read(next_preview).unwrap().unwrap();
    db.execute(&Caller::Runtime, &FinishPreview { outbox_id: job.outbox_id, result: PreviewResult::Written }).unwrap();
    assert_eq!(db.read(load_project).unwrap().unwrap().previewed, "first");
    assert_eq!(db.read(preview_stopped).unwrap(), None);
    assert_eq!(db.read(next_preview).unwrap(), None);
}
