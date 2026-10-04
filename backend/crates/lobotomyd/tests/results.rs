//! Store work driven directly, without the scheduler: reopening and the verification site (#12).
//! Needs `node` and `git` on PATH.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use lobotomy_core::capture::pending_captures;
use lobotomy_core::project::{Check, EditProjectConfig, current_config};
use lobotomy_core::report::{OrgReport, ReportStatus};
use lobotomy_core::task::{Abandon, Reopen, StartAttempt};
use lobotomy_core::turn::{EndTurn, Outcome, RegisterTurn, SessionIdentified};
use lobotomy_core::verify::{StartVerification, VerificationState, latest_candidate_commit, latest_verification};
use lobotomy_core::workspace::materializing;
use lobotomy_core::{Caller, Db};
use lobotomyd::project::Project;
use lobotomyd::results;

/// A turn that writes `work.txt` and reports done; returns the candidate's commit.
async fn make_candidate(project: &Arc<Project>, text: &str) -> String {
    let db = &project.db;
    let t = db.execute(&Caller::Runtime, &RegisterTurn { role: "Malkuth".into() }).unwrap();
    let identified = SessionIdentified { turn_id: t.turn_id.clone(), native_id: format!("native-{text}") };
    db.execute(&Caller::Runtime, &identified).unwrap();
    std::fs::write(project.slot_dir("worker").join("work.txt"), text).unwrap();
    let report = OrgReport { title: text.into(), body: text.into(), status: ReportStatus::Done, blocked_on: None };
    db.execute(&Caller::Role { role: "Malkuth".into(), turn_id: t.turn_id.clone() }, &report).unwrap();
    db.execute(&Caller::Runtime, &EndTurn { turn_id: t.turn_id, outcome: Outcome::Completed, failure: None }).unwrap();
    for capture in db.read(pending_captures).unwrap() {
        results::capture(project.clone(), capture).await.unwrap();
    }
    let task = last(db).unwrap().task_id.unwrap();
    db.read(|c| latest_candidate_commit(c, &task)).unwrap().unwrap()
}

fn abandon_and_reopen(db: &Db, task: &str, round: u32) {
    let abandon = Abandon { request_id: format!("abandon-{round}"), task_id: task.into(), reason: String::new() };
    db.execute(&Caller::User, &abandon).unwrap();
    db.execute(&Caller::User, &Reopen { request_id: format!("reopen-{round}"), task_id: task.into() }).unwrap();
}

async fn materialize_planned(project: &Arc<Project>) {
    for ws in project.db.read(materializing).unwrap() {
        results::materialize(project.clone(), ws).await.unwrap();
    }
}

/// #12: with the integration version unchanged, the second reopen starts from the newest
/// candidate, not from the first one.
#[tokio::test]
async fn a_second_reopen_starts_from_the_latest_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let project = open_project(dir.path(), fake_codex()).await;
    let db = &project.db;
    let task = create_task(db, "reopen twice");
    start_attempt(&project, &task).await;
    let a = make_candidate(&project, "candidate A").await;

    abandon_and_reopen(db, &task, 1);
    let start_a = results::code_start(&project, &task).await.unwrap().unwrap();
    // The same inputs give the same start.
    assert_eq!(results::code_start(&project, &task).await.unwrap().unwrap(), start_a);
    db.execute(&Caller::Runtime, &StartAttempt { task_id: task.clone(), code_start: Some(start_a.clone()) }).unwrap();
    materialize_planned(&project).await;
    assert_eq!(std::fs::read_to_string(project.slot_dir("worker").join("work.txt")).unwrap(), "candidate A");
    let b = make_candidate(&project, "candidate B").await;
    assert_ne!(a, b);

    abandon_and_reopen(db, &task, 2);
    let start_b = results::code_start(&project, &task).await.unwrap().unwrap();
    assert_ne!(start_b.commit, start_a.commit);
    db.execute(&Caller::Runtime, &StartAttempt { task_id: task.clone(), code_start: Some(start_b) }).unwrap();
    materialize_planned(&project).await;
    assert_eq!(std::fs::read_to_string(project.slot_dir("worker").join("work.txt")).unwrap(), "candidate B");
}

async fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(Instant::now() < deadline, "timed out waiting for {}", path.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// #12: a check of an abandoned task that is still running cannot write into the next task's
/// verification. Verifications take turns on the site: B waits until A's check has ended, then
/// writes its own candidate and checks it.
#[tokio::test]
async fn a_check_of_an_abandoned_task_cannot_touch_the_next_verification() {
    let dir = tempfile::tempdir().unwrap();
    let project = open_project(dir.path(), fake_codex()).await;
    let db = &project.db;
    let marker = |name: &str| dir.path().join(name);
    let (started_a, release_a, started_b) = (marker("started-A"), marker("release-A"), marker("started-B"));
    let script = dir.path().join("check.mjs");
    let paths = serde_json::to_string(&[&started_a, &release_a, &started_b]).unwrap();
    std::fs::write(
        &script,
        format!(
            r#"import fs from 'node:fs';
const [startedA, releaseA, startedB] = {paths};
const original = fs.readFileSync('work.txt', 'utf8');
if (original === 'candidate A') {{
  fs.writeFileSync(startedA, '');
  const end = Date.now() + 30000;
  while (!fs.existsSync(releaseA)) {{
    if (Date.now() > end) throw Error('never released');
    await new Promise(r => setTimeout(r, 20));
  }}
  fs.writeFileSync('work.txt', 'old task contamination');
}} else {{
  fs.writeFileSync(startedB, original);
  const current = fs.readFileSync('work.txt', 'utf8');
  console.log(JSON.stringify({{ original, current }}));
  process.exit(current === original ? 0 : 7);
}}
"#
        ),
    )
    .unwrap();
    let (version, mut config) = db.read(current_config).unwrap();
    let command = format!("node \"{}\"", script.to_string_lossy().replace('\\', "/"));
    config.checks = vec![Check { command, timeout_secs: 60 }];
    db.execute(&Caller::User, &EditProjectConfig { request_id: "checks".into(), expected_version: version, config }).unwrap();

    let a = create_task(db, "old verifier");
    start_attempt(&project, &a).await;
    make_candidate(&project, "candidate A").await;
    let va = db.execute(&Caller::Runtime, &StartVerification { task_id: a.clone() }).unwrap();
    let first = tokio::spawn(results::verify(project.clone(), va));
    wait_for_file(&started_a).await;

    db.execute(&Caller::User, &Abandon { request_id: "abandon-A".into(), task_id: a, reason: String::new() }).unwrap();
    let b = create_task(db, "next verifier");
    start_attempt(&project, &b).await;
    make_candidate(&project, "candidate B").await;
    let vb = db.execute(&Caller::Runtime, &StartVerification { task_id: b.clone() }).unwrap();
    let second = tokio::spawn(results::verify(project.clone(), vb));

    // B waits for the site while A's check runs.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!started_b.exists(), "B checked while A's check still ran");
    assert_eq!(std::fs::read_to_string(project.verify_dir().join("work.txt")).unwrap(), "candidate A");

    std::fs::write(&release_a, "").unwrap();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(std::fs::read_to_string(&started_b).unwrap(), "candidate B");
    let v = db.read(|c| latest_verification(c, &b)).unwrap().unwrap();
    assert_eq!(v.state, VerificationState::Passed, "A's late write did not reach B's check");
}
