//! #67: observations gate starts; only explicit adoption at a savepoint creates a revision.
mod common;
use common::*;
use lobotomy_core::display::*;
use lobotomy_core::project::{Check, EditProjectConfig, current_config, require_project};
use lobotomy_core::task::{SetPaused, StartAttempt};
use lobotomy_core::turn::RegisterTurn;
use lobotomy_core::verify::{Accept, FinishPreview, PreviewResult, next_preview};
use lobotomy_core::{Caller, Db};

fn changed(db: &Db, target: &str) {
    let p = db.read(require_project).unwrap();
    db.execute(
        &Caller::Runtime,
        &ObserveDisplay { previewed: p.previewed, state: DisplayState::Changed { commit: target.into() } },
    )
    .unwrap();
}
fn cmd(db: &Db) -> AdoptDisplay {
    AdoptDisplay {
        request_id: "adopt".into(),
        target: "external".into(),
        expected_rev: db.read(require_project).unwrap().integration_rev,
    }
}
fn finish(db: &Db, id: &str) {
    db.execute(&Caller::Runtime, &FinishAdoption { id: id.into(), checks: vec![], error: None }).unwrap();
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
    let id = db.execute(&Caller::User, &cmd(&db)).unwrap();
    assert_eq!(
        rejection(db.execute(&Caller::Runtime, &lobotomy_core::workspace::AlignIdleSlot { role: ROLE.into() })),
        "adoption_pending"
    );
    finish(&db, &id);
    assert!(db.read(next_preview).unwrap().is_none());
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
fn saved_but_undecided_taskless_changes_also_block_adoption() {
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
            capture_id: capture.id,
            result: CaptureResult::Pinned { commit: "saved".into(), changed: vec!["work.txt".into()] },
        },
    )
    .unwrap();
    changed(&db, "external");
    assert_eq!(rejection(db.execute(&Caller::User, &cmd(&db))), "not_at_savepoint");
}
