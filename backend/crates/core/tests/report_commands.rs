//! A trial is optional metadata on the done that owns an attempt's candidate.

mod common;

use common::{ROLE, create, db, end_turn, pin_captures, register, start};
use lobotomy_core::capture::{CaptureResult, FinishCapture, pending_captures};
use lobotomy_core::report::{OrgReport, ReportEffect, ReportStatus};
use lobotomy_core::task::{Phase, Trial, latest_attempt, load_task, open_attempt};
use lobotomy_core::turn::{Continue, load_turn};
use lobotomy_core::{Caller, Db};
use serde_json::json;

fn report(status: ReportStatus, trial: Option<Trial>) -> OrgReport {
    OrgReport { title: "完成".into(), body: "体验页面".into(), status, blocked_on: None, trial }
}

fn trial(command: &str) -> Trial {
    Trial { command: command.into(), purpose: "体验页面".into() }
}

fn caller(turn_id: &str) -> Caller {
    Caller::Role { role: ROLE.into(), turn_id: turn_id.into() }
}

#[test]
fn the_first_done_keeps_its_trial_with_the_candidate_and_after_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lobotomy.db");
    let db = common::onboard(Db::open(&path).unwrap());
    let task = create(&db, "c1");
    start(&db, &task);
    let turn = register(&db);
    let suggested = trial("npm run dev");
    assert_eq!(
        db.execute(&caller(&turn.turn_id), &report(ReportStatus::Done, Some(suggested.clone()))).unwrap(),
        ReportEffect::Recorded
    );
    assert_eq!(db.read(|c| open_attempt(c, &task)).unwrap().unwrap().trial, Some(suggested.clone()));
    end_turn(&db, &turn.turn_id);
    pin_captures(&db);
    // The attempt owns the data even if the transcript's command arguments are unavailable.
    db.write(|tx| Ok(tx.execute("UPDATE command_record SET args = '{}' WHERE name = 'org_report'", [])?)).unwrap();
    drop(db);

    let db = Db::open(&path).unwrap();
    let attempt = db.read(|c| latest_attempt(c, &task)).unwrap().unwrap();
    assert!(attempt.candidate_id.is_some());
    assert_eq!(attempt.trial, Some(suggested));
    let serialized = serde_json::to_value(attempt).unwrap();
    assert_eq!(serialized["trial"], json!({ "command": "npm run dev", "purpose": "体验页面" }));
}

#[test]
fn older_reports_without_trial_still_complete_the_attempt() {
    let db = db();
    let task = create(&db, "c1");
    start(&db, &task);
    let turn = register(&db);
    let legacy: OrgReport =
        serde_json::from_value(json!({ "title": "完成", "body": "已验证", "status": "done" })).unwrap();
    assert_eq!(legacy.trial, None);
    assert_eq!(db.execute(&caller(&turn.turn_id), &legacy).unwrap(), ReportEffect::Recorded);
    end_turn(&db, &turn.turn_id);
    pin_captures(&db);
    let attempt = db.read(|c| latest_attempt(c, &task)).unwrap().unwrap();
    assert!(attempt.candidate_id.is_some());
    assert_eq!(attempt.trial, None);
}

#[test]
fn repeated_done_never_replaces_or_adds_a_trial() {
    for first in [None, Some(trial("npm run dev"))] {
        let db = db();
        let task = create(&db, "c1");
        start(&db, &task);
        let turn = register(&db);
        let caller = caller(&turn.turn_id);
        db.execute(&caller, &report(ReportStatus::Done, first.clone())).unwrap();
        let first_done_at = db.read(|c| load_turn(c, &turn.turn_id)).unwrap().done_at;
        assert_eq!(
            db.execute(&caller, &report(ReportStatus::Done, Some(trial("other command")))).unwrap(),
            ReportEffect::Unchanged
        );
        assert_eq!(db.read(|c| open_attempt(c, &task)).unwrap().unwrap().trial, first);
        assert_eq!(db.read(|c| load_turn(c, &turn.turn_id)).unwrap().done_at, first_done_at);
    }
}

#[test]
fn progress_and_blocked_never_save_a_candidate_trial_or_launch_its_command() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("must-not-run.txt");
    let suggested = trial(&format!("echo trial > \"{}\"", marker.display()));
    let db = db();
    let task = create(&db, "c1");
    start(&db, &task);
    let turn = register(&db);
    for status in [ReportStatus::Progress, ReportStatus::Blocked] {
        assert_eq!(
            db.execute(&caller(&turn.turn_id), &report(status, Some(suggested.clone()))).unwrap(),
            ReportEffect::Recorded
        );
        let attempt = db.read(|c| open_attempt(c, &task)).unwrap().unwrap();
        assert_eq!((attempt.done_turn_id, attempt.done_summary, attempt.trial), (None, None, None));
        assert_eq!(attempt.candidate_id, None);
        assert_eq!(db.read(|c| load_task(c, &task)).unwrap().phase, Phase::Executing);
        assert!(db.read(pending_captures).unwrap().is_empty());
        assert!(!marker.exists());
    }
    // A done also just saves metadata; no command is launched by reporting it.
    db.execute(&caller(&turn.turn_id), &report(ReportStatus::Done, Some(suggested))).unwrap();
    assert!(!marker.exists());
}

#[test]
fn a_late_done_cannot_save_a_trial() {
    let db = db();
    let task = create(&db, "c1");
    start(&db, &task);
    let turn = register(&db);
    end_turn(&db, &turn.turn_id);
    assert_eq!(
        db.execute(&caller(&turn.turn_id), &report(ReportStatus::Done, Some(trial("npm run dev")))).unwrap(),
        ReportEffect::Late
    );
    assert_eq!(db.read(|c| open_attempt(c, &task)).unwrap().unwrap().trial, None);
}

#[test]
fn continuing_a_stopped_done_clears_its_trial_along_with_the_summary() {
    let db = db();
    let task = create(&db, "c1");
    start(&db, &task);
    let turn = register(&db);
    db.execute(&caller(&turn.turn_id), &report(ReportStatus::Done, Some(trial("npm run dev")))).unwrap();
    end_turn(&db, &turn.turn_id);
    let capture = db.read(pending_captures).unwrap().pop().unwrap();
    db.execute(
        &Caller::Runtime,
        &FinishCapture {
            capture_id: capture.id,
            result: CaptureResult::Oversized { files: vec![], total_bytes: 1 << 40 },
        },
    )
    .unwrap();
    let next = db.execute(&Caller::User, &Continue { request_id: "k1".into(), role: ROLE.into() }).unwrap();
    let attempt = db.read(|c| open_attempt(c, &task)).unwrap().unwrap();
    assert_eq!((attempt.done_turn_id, attempt.done_summary, attempt.trial), (None, None, None));
    db.execute(&caller(&next.turn_id), &report(ReportStatus::Done, None)).unwrap();
    end_turn(&db, &next.turn_id);
    pin_captures(&db);
    assert_eq!(db.read(|c| latest_attempt(c, &task)).unwrap().unwrap().trial, None);
}

#[test]
fn upgrading_a_database_preserves_existing_attempts_without_a_trial() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lobotomy.db");
    let db = common::onboard(Db::open(&path).unwrap());
    let task = create(&db, "c1");
    start(&db, &task);
    let turn = register(&db);
    db.execute(&caller(&turn.turn_id), &report(ReportStatus::Done, None)).unwrap();
    end_turn(&db, &turn.turn_id);
    pin_captures(&db);
    drop(db);
    // The database as migration 8 left it.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "ALTER TABLE event DROP COLUMN command_id; ALTER TABLE attempt DROP COLUMN trial; PRAGMA user_version = 8;",
    )
    .unwrap();
    drop(conn);

    let db = Db::open(&path).unwrap();
    let attempt = db.read(|c| latest_attempt(c, &task)).unwrap().unwrap();
    assert_eq!(attempt.done_summary.as_deref(), Some("完成\n\n体验页面"));
    assert!(attempt.candidate_id.is_some());
    assert_eq!(attempt.trial, None);
}
