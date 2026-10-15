//! #67: observations gate starts; only explicit adoption at a savepoint creates a revision.
mod common;
use common::*;
use lobotomy_core::display::*;
use lobotomy_core::project::{Check, EditProjectConfig, current_config, require_project};
use lobotomy_core::task::{Abandon, AdoptOutsideChanges, Reopen, SetPaused, StartAttempt};
use lobotomy_core::turn::RegisterTurn;
use lobotomy_core::verify::{Accept, FinishPreview, PreviewResult, next_preview};
use lobotomy_core::{Caller, Db};

fn changed(db: &Db, target: &str) {
    db.execute(&Caller::Runtime, &ObserveDisplay { state: DisplayState::Changed { commit: target.into() } }).unwrap();
}
fn cmd(db: &Db) -> AdoptDisplay {
    AdoptDisplay {
        request_id: "adopt".into(),
        acknowledged: vec![],
        target: "external".into(),
        expected_rev: db.read(require_project).unwrap().integration_rev,
    }
}
fn finish(db: &Db, id: &str) {
    db.execute(&Caller::Runtime, &FinishAdoption { id: id.into(), checks: vec![], blobs: vec![], error: None })
        .unwrap();
}

#[test]
fn observation_allows_queueing_but_not_starting_or_adopting_implicitly() {
    let db = db();
    changed(&db, "external");
    let task = create(&db, "queued");
    assert_eq!(
        rejection(db.execute(&Caller::Runtime, &StartAttempt { task_id: task.clone(), code_start: None })),
        "display_changed"
    );
    assert_eq!(db.read(require_project).unwrap().integration, BASE);
    let request = cmd(&db);
    let id = db.execute(&Caller::User, &request).unwrap();
    assert_eq!(db.execute(&Caller::User, &request).unwrap(), id);
    assert_eq!(db.read(require_project).unwrap().integration, BASE);
    let fresh = AdoptDisplay { request_id: "double-click".into(), ..request };
    assert_eq!(rejection(db.execute(&Caller::User, &fresh)), "adoption_pending");
    assert_eq!(
        rejection(db.execute(&Caller::Runtime, &StartAttempt { task_id: task.clone(), code_start: None })),
        "adoption_pending"
    );
    finish(&db, &id);
    finish(&db, &id);
    let p = db.read(require_project).unwrap();
    assert_eq!((p.integration.as_str(), p.previewed.as_str(), p.integration_rev), ("external", "external", 2));
    start(&db, &task);
    let attempts = db.read(|c| lobotomy_core::task::list_attempts(c, &task)).unwrap();
    assert_eq!(attempts[0].code_start.as_deref(), Some("external"));
}

#[test]
fn pause_is_not_a_savepoint_and_verifying_or_accepting_candidates_block_adoption() {
    let db = db();
    let task = create(&db, "task");
    start(&db, &task);
    db.execute(&Caller::User, &SetPaused { request_id: "pause".into(), task_id: task, paused: true }).unwrap();
    changed(&db, "external");
    assert_eq!(rejection(db.execute(&Caller::User, &cmd(&db))), "not_at_savepoint");
    let db = common::db();
    let (task, commit) = candidate(&db);
    changed(&db, "external");
    assert_eq!(rejection(db.execute(&Caller::User, &cmd(&db))), "not_at_savepoint");
    verify(&db, &task, &commit, vec![], vec![]);
    assert_eq!(rejection(db.execute(&Caller::User, &cmd(&db))), "not_at_savepoint");
}

#[test]
fn registered_turn_and_unsaved_capture_are_not_savepoints() {
    let db = db();
    idle_slot(&db);
    send(&db, "message", None, "hello");
    let t = register(&db);
    changed(&db, "external");
    assert_eq!(rejection(db.execute(&Caller::User, &cmd(&db))), "not_at_savepoint");
    end_turn(&db, &t.turn_id);
    assert_eq!(rejection(db.execute(&Caller::User, &cmd(&db))), "not_at_savepoint");
    pin_captures(&db);
    let id = db.execute(&Caller::User, &cmd(&db)).unwrap();
    assert_eq!(rejection(db.execute(&Caller::Runtime, &RegisterTurn { role: ROLE.into() })), "adoption_pending");
    finish(&db, &id);
}

#[test]
fn version_changes_checks_and_failures_leave_the_old_baseline() {
    for change in ["config", "head", "internal", "checks"] {
        let db = db();
        changed(&db, "external");
        if change == "checks" {
            let (_, mut config) = db.read(current_config).unwrap();
            config.checks.push(Check { command: "check".into(), timeout_secs: 1 });
            db.execute(&Caller::User, &EditProjectConfig { request_id: "config".into(), expected_version: 1, config })
                .unwrap();
        }
        let id = db.execute(&Caller::User, &cmd(&db)).unwrap();
        match change {
            "config" => {
                let (_, config) = db.read(current_config).unwrap();
                db.execute(
                    &Caller::User,
                    &EditProjectConfig { request_id: "config".into(), expected_version: 1, config },
                )
                .unwrap();
            }
            "head" => changed(&db, "other"),
            "internal" => {
                db.write(|tx| Ok(tx.execute("UPDATE project SET integration_rev=2", [])?)).unwrap();
            }
            _ => {}
        }
        finish(&db, &id);
        assert_eq!(db.read(require_project).unwrap().integration, BASE, "{change}");
        assert_eq!(db.read(|c| load(c, &id)).unwrap().state, "failed", "{change}");
    }
}

#[test]
fn adoption_supersedes_previews_and_preserves_accepted_task_history() {
    let db = db();
    let (task, _) = candidate(&db);
    let v = verify(&db, &task, "internal", vec![], vec![]);
    db.execute(
        &Caller::User,
        &Accept {
            request_id: "accept".into(),
            task_id: task.clone(),
            verification_id: v.clone(),
            expected_integration: BASE.into(),
            criteria_version: 1,
            dropping: vec![],
        },
    )
    .unwrap();
    let old_preview = db.read(next_preview).unwrap().unwrap();
    changed(&db, "external");
    let request = cmd(&db);
    assert_eq!(rejection(db.execute(&Caller::User, &request)), "unpreviewed_publications");
    let unseen = db.read(view).unwrap().unpreviewed;
    assert_eq!(unseen.len(), 1);
    assert_eq!((unseen[0].task_id.as_str(), unseen[0].commit.as_str()), (task.as_str(), "internal"));
    let id = db.execute(&Caller::User, &AdoptDisplay { acknowledged: vec!["internal".into()], ..request }).unwrap();
    assert_eq!(
        rejection(db.execute(&Caller::Runtime, &lobotomy_core::workspace::AlignIdleSlot { role: ROLE.into() })),
        "adoption_pending"
    );
    finish(&db, &id);
    assert!(db.read(next_preview).unwrap().is_none());
    assert!(db.read(view).unwrap().unpreviewed.is_empty());
    assert_eq!(
        rejection(db.execute(
            &Caller::Runtime,
            &FinishPreview { outbox_id: old_preview.outbox_id, result: PreviewResult::Written }
        )),
        "not_pending"
    );
    assert_eq!(db.read(|c| lobotomy_core::task::load_task(c, &task)).unwrap().phase, lobotomy_core::task::Phase::Done);
    assert_eq!(
        db.read(|c| lobotomy_core::verify::load_verification(c, &v)).unwrap().commit_id.as_deref(),
        Some("internal")
    );
    let count: i64 = db.read(|c| Ok(c.query_row("SELECT count(*) FROM publication", [], |r| r.get(0))?)).unwrap();
    assert_eq!(count, 1, "adoption is not a fake worker publication");
}

#[test]
fn taskless_changes_block_adoption_until_the_user_decides() {
    use lobotomy_core::capture::{CaptureResult, FinishCapture, pending_captures};
    let db = db();
    idle_slot(&db);
    send(&db, "message", None, "work outside a task");
    let turn = register(&db);
    end_turn(&db, &turn.turn_id);
    let capture = db.read(pending_captures).unwrap().pop().unwrap();
    db.execute(
        &Caller::Runtime,
        &FinishCapture {
            capture_id: capture.id.clone(),
            result: CaptureResult::Pinned { commit: "saved".into(), changed: vec!["work.txt".into()] },
        },
    )
    .unwrap();
    changed(&db, "external");
    assert_eq!(rejection(db.execute(&Caller::User, &cmd(&db))), "not_at_savepoint");
    db.execute(
        &Caller::User,
        &AdoptOutsideChanges {
            request_id: "queue-outside".into(),
            capture_id: capture.id,
            title: "继续任务外改动".into(),
            body: "finish the saved changes".into(),
            criteria: "done".into(),
        },
    )
    .unwrap();
    let id = db.execute(&Caller::User, &cmd(&db)).unwrap();
    finish(&db, &id);
    assert_eq!(db.read(require_project).unwrap().integration, "external");
}

#[test]
fn queued_candidates_do_not_block_admission_or_final_cas() {
    let db = db();
    let (task, candidate) = candidate(&db);
    db.execute(
        &Caller::User,
        &Abandon { request_id: "close".into(), task_id: task.clone(), reason: "keep history".into() },
    )
    .unwrap();
    changed(&db, "external");
    db.execute(&Caller::User, &Reopen { request_id: "reopen".into(), task_id: task.clone() }).unwrap();
    let id = db.execute(&Caller::User, &cmd(&db)).unwrap();
    finish(&db, &id);
    assert_eq!(db.read(|c| load(c, &id)).unwrap().state, "applied");
    assert_eq!(db.read(require_project).unwrap().integration, "external");
    assert_eq!(db.read(|c| lobotomy_core::verify::work_so_far(c, &task)).unwrap(), Some(candidate));
}

#[test]
fn adoption_errors_follow_the_observed_target() {
    let db = db();
    changed(&db, "external");
    let id = db.execute(&Caller::User, &cmd(&db)).unwrap();
    db.execute(
        &Caller::Runtime,
        &FinishAdoption { id, checks: vec![], blobs: vec![], error: Some("target failed".into()) },
    )
    .unwrap();
    assert_eq!(db.read(view).unwrap().error.as_deref(), Some("target failed"));
    changed(&db, "next");
    assert!(db.read(view).unwrap().error.is_none());
    changed(&db, "external");
    assert_eq!(db.read(view).unwrap().error.as_deref(), Some("target failed"));
    db.execute(&Caller::Runtime, &ObserveDisplay { state: DisplayState::Current }).unwrap();
    assert!(db.read(view).unwrap().error.is_none());
}

#[test]
fn a_preview_receipt_invalidates_an_old_unpreviewed_confirmation() {
    let db = db();
    let (task, _) = candidate(&db);
    let v = verify(&db, &task, "internal", vec![], vec![]);
    db.execute(
        &Caller::User,
        &Accept {
            request_id: "accept".into(),
            task_id: task,
            verification_id: v,
            expected_integration: BASE.into(),
            criteria_version: 1,
            dropping: vec![],
        },
    )
    .unwrap();
    let old_preview = db.read(next_preview).unwrap().unwrap();
    changed(&db, "external");
    let confirmed = AdoptDisplay { acknowledged: vec!["internal".into()], ..cmd(&db) };
    db.execute(&Caller::Runtime, &FinishPreview { outbox_id: old_preview.outbox_id, result: PreviewResult::Written })
        .unwrap();
    changed(&db, "external");
    assert!(db.read(view).unwrap().unpreviewed.is_empty());
    assert_eq!(rejection(db.execute(&Caller::User, &confirmed)), "unpreviewed_publications");
    let id = db.execute(&Caller::User, &AdoptDisplay { acknowledged: vec![], ..confirmed }).unwrap();
    finish(&db, &id);
    assert_eq!(db.read(require_project).unwrap().integration, "external");
}
