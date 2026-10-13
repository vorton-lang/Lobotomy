//! #67: real local Git repositories; no network remotes or model subscriptions.
mod common;
use common::*;
use lobotomy_core::display::{self as state, AdoptDisplay, DisplayState};
use lobotomy_core::project::{Check, EditProjectConfig, current_config, require_project};
use lobotomy_core::task::{StartAttempt, load_task};
use lobotomy_core::{Caller, Db};
use lobotomyd::{display, project::Project};
use std::path::Path;
use std::sync::Arc;

fn commit(repo: &Path, file: &str, text: &str) -> String {
    std::fs::write(repo.join(file), text).unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "--quiet", "-m", text]);
    git(repo, &["rev-parse", "HEAD"])
}
fn request(project: &Project, target: &str) -> AdoptDisplay {
    AdoptDisplay {
        request_id: "adoption".into(),
        target: target.into(),
        expected_rev: project.db.read(require_project).unwrap().integration_rev,
    }
}
async fn observe(project: &Arc<Project>) -> DisplayState {
    let _gate = project.activity.lock().await;
    display::observe(project).await.unwrap()
}
fn checks(db: &Db, command: String) {
    let (version, mut config) = db.read(current_config).unwrap();
    config.checks = vec![Check { command, timeout_secs: 30 }];
    db.execute(&Caller::User, &EditProjectConfig { request_id: "checks".into(), expected_version: version, config })
        .unwrap();
}

#[tokio::test]
async fn idle_observation_blocks_start_and_adoption_then_starts_from_external_tree() {
    let dir = tempfile::tempdir().unwrap();
    let p = open_project(dir.path(), fake_codex()).await;
    let repo = user_repo(dir.path());
    let initial = p.db.read(require_project).unwrap().integration;
    let target = commit(&repo, "external.txt", "external");
    assert_eq!(observe(&p).await, DisplayState::Changed { commit: target.clone() });
    let task = create_task(&p, "queued");
    assert_eq!(
        p.db.execute(&Caller::Runtime, &StartAttempt { task_id: task.clone(), code_start: None }).unwrap_err().code(),
        Some("display_changed")
    );
    let cmd = request(&p, &target);
    let id = display::begin(&p, cmd.clone()).await.unwrap();
    assert_eq!(display::begin(&p, cmd.clone()).await.unwrap(), id, "a lost reply is idempotent");
    display::adopt(p.clone(), id.clone()).await.unwrap();
    assert_eq!(display::begin(&p, cmd).await.unwrap(), id);
    assert_eq!(p.db.read(|c| state::load(c, &id)).unwrap().state, "applied");
    assert_eq!(p.store.pinned(&format!("{id}-previous")).unwrap(), Some(initial));
    assert_eq!(p.db.read(require_project).unwrap().integration, target);
    start_attempt(&p, &task).await;
    assert_eq!(std::fs::read_to_string(p.slot_dir("worker").join("external.txt")).unwrap(), "external");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), target, "adoption never writes the display");
}

#[tokio::test]
async fn dirty_wrong_branch_detached_and_read_failure_are_visible_and_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = open_project(dir.path(), fake_codex()).await;
    let repo = user_repo(dir.path());
    let target = commit(&repo, "external.txt", "external");
    for kind in ["dirty", "branch", "detached", "missing"] {
        match kind {
            "dirty" => std::fs::write(repo.join("unfinished.txt"), "keep").unwrap(),
            "branch" => {
                std::fs::remove_file(repo.join("unfinished.txt")).unwrap();
                git(&repo, &["switch", "-c", "other"]);
            }
            "detached" => {
                git(&repo, &["checkout", "--detach"]);
            }
            "missing" => {
                std::fs::rename(&repo, dir.path().join("aside")).unwrap();
            }
            _ => unreachable!(),
        }
        let DisplayState::Blocked { reason } = observe(&p).await else {
            panic!("{kind} must be visible");
        };
        assert!(!reason.is_empty());
        assert!(display::begin(&p, request(&p, &target)).await.is_err(), "{kind}");
        assert!(p.db.read(state::pending).unwrap().is_none());
    }
}

#[tokio::test]
async fn checks_fail_without_adoption_and_output_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let p = open_project(dir.path(), fake_codex()).await;
    let before = p.db.read(require_project).unwrap();
    let target = commit(&user_repo(dir.path()), "external.txt", "external");
    checks(&p.db, "node -e \"console.log('check evidence');process.exit(7)\"".into());
    let id = display::begin(&p, request(&p, &target)).await.unwrap();
    display::adopt(p.clone(), id.clone()).await.unwrap();
    let a = p.db.read(|c| state::load(c, &id)).unwrap();
    assert_eq!(a.state, "failed");
    assert!(a.error.unwrap().contains("check evidence"));
    assert_eq!(p.db.read(require_project).unwrap(), before);
    let checks: String =
        p.db.read(|c| Ok(c.query_row("SELECT checks FROM display_adoption WHERE id=?1", [id], |r| r.get(0))?)).unwrap();
    assert!(checks.contains("check evidence"));
}

#[tokio::test]
async fn a_check_that_changes_display_head_cannot_publish_its_old_target() {
    let dir = tempfile::tempdir().unwrap();
    let p = open_project(dir.path(), fake_codex()).await;
    let before = p.db.read(require_project).unwrap();
    let repo = user_repo(dir.path());
    let target = commit(&repo, "external.txt", "external");
    let script = dir.path().join("change.mjs");
    let repo_json = serde_json::to_string(&repo).unwrap();
    std::fs::write(&script,format!("import fs from 'node:fs'; import {{execFileSync}} from 'node:child_process'; const repo={repo_json}; fs.writeFileSync(repo+'/new.txt','new'); for (const args of [['add','.'],['commit','-qm','new']]) execFileSync('git',['-C',repo,...args]);")).unwrap();
    checks(&p.db, format!("node \"{}\"", script.display()));
    let id = display::begin(&p, request(&p, &target)).await.unwrap();
    display::adopt(p.clone(), id.clone()).await.unwrap();
    assert_eq!(p.db.read(|c| state::load(c, &id)).unwrap().state, "failed");
    assert_eq!(p.db.read(require_project).unwrap(), before);
    assert_eq!(p.store.pinned(&id).unwrap(), Some(target));
    assert!(repo.join("new.txt").exists());
}

#[tokio::test]
async fn pending_intent_survives_reopen_but_never_follows_a_new_head() {
    for moved in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let p = open_project(dir.path(), fake_codex()).await;
        let before = p.db.read(require_project).unwrap();
        let repo = user_repo(dir.path());
        let target = commit(&repo, "external.txt", "external");
        let id = display::begin(&p, request(&p, &target)).await.unwrap();
        // Crash boundary: import/pin succeeded, final business transaction has not happened.
        p.store.import(&repo, "main", Some(&target), &id).unwrap();
        drop(p);
        if moved {
            commit(&repo, "later.txt", "later");
        }
        let p = open_project(dir.path(), fake_codex()).await;
        lobotomyd::scheduler::recover(&p).await.unwrap();
        display::adopt(p.clone(), id.clone()).await.unwrap();
        let a = p.db.read(|c| state::load(c, &id)).unwrap();
        assert_eq!(a.state, if moved { "failed" } else { "applied" });
        assert_eq!(
            p.db.read(require_project).unwrap().integration,
            if moved { before.integration } else { target.clone() }
        );
        assert_eq!(p.store.pinned(&id).unwrap(), Some(target));
    }
}

#[tokio::test]
async fn live_job_and_click_with_stale_internal_version_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = open_project(dir.path(), fake_codex()).await;
    let target = commit(&user_repo(dir.path()), "external.txt", "external");
    let job = tokio::spawn(std::future::pending::<()>());
    p.jobs.lock().unwrap().insert("preview".into(), job.abort_handle());
    assert!(display::begin(&p, request(&p, &target)).await.unwrap_err().to_string().contains("实际退出"));
    p.jobs.lock().unwrap().remove("preview");
    job.abort();
    // EndTurn can commit before the runner has released its process/IO handles. SQL alone
    // reports a savepoint, but admission must still wait for the actual runner to finish.
    p.db.read(state::require_savepoint).unwrap();
    p.running.lock().unwrap().insert(
        "ending-turn".into(),
        lobotomyd::instance::RunningTurn {
            pid: None,
            interrupt_requested: false,
            stop: tokio_util::sync::CancellationToken::new(),
            task: None,
        },
    );
    assert!(display::begin(&p, request(&p, &target)).await.unwrap_err().to_string().contains("实际退出"));
    p.running.lock().unwrap().remove("ending-turn");
    let mut cmd = request(&p, &target);
    cmd.expected_rev += 1;
    assert!(display::begin(&p, cmd).await.is_err());
    assert!(p.db.read(state::pending).unwrap().is_none());
}

#[tokio::test]
async fn scheduler_observes_external_commit_before_starting_an_already_queued_task() {
    let dir = tempfile::tempdir().unwrap();
    let p = open_project(dir.path(), fake_codex()).await;
    let task = create_task(&p, "queued before external change");
    let target = commit(&user_repo(dir.path()), "external.txt", "external");
    drop(p);
    let started = start(dir.path()).await;
    wait_until(|| matches!(started.project.db.read(state::observation).unwrap(), DisplayState::Changed { .. })).await;
    assert_eq!(started.project.db.read(|c| load_task(c, &task)).unwrap().phase, lobotomy_core::task::Phase::Queued);
    // Wait for the idle slot, whose materialization is independent of external state.
    wait_until(|| {
        started.project.jobs.lock().unwrap().is_empty()
            && started.project.db.read(lobotomy_core::workspace::materializing).unwrap().is_empty()
    })
    .await;
    let id = display::begin(&started.project, request(&started.project, &target)).await.unwrap();
    wait_until(|| started.project.db.read(|c| state::load(c, &id)).unwrap().state != "pending").await;
    assert_eq!(started.project.db.read(|c| state::load(c, &id)).unwrap().state, "applied");
    wait_until(|| {
        started.project.db.read(|c| load_task(c, &task)).unwrap().phase != lobotomy_core::task::Phase::Queued
    })
    .await;
    started.shutdown(std::time::Duration::from_secs(5)).await;
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("state transition timed out");
}
