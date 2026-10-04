//! Helpers for the backend tests with a fake Codex (tests/fixtures/fake-codex.mjs, run by Node).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lobotomy_core::task::{CreateTask, Phase, StartAttempt, load_task};
use lobotomy_core::turn::{Turn, TurnState, last_turn};
use lobotomy_core::workspace::materializing;
use lobotomy_core::{Caller, Db};
use lobotomyd::Backend;
use lobotomyd::host::{Cli, HarnessConfig, Host};
use lobotomyd::project::Project;

/// The fake Codex. `flags` go before Codex's own arguments, such as `--fake-hang`.
pub fn fake_codex_with(flags: &[&str]) -> HarnessConfig {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("fake-codex.mjs");
    let mut command = vec!["node".to_owned(), script.to_string_lossy().into_owned()];
    command.extend(flags.iter().map(|f| (*f).to_owned()));
    HarnessConfig {
        codex: Cli { command, reasoning_effort: None },
        interrupt_helper: vec![env!("CARGO_BIN_EXE_lobotomyd").into(), "ctrl-c".into()],
        probe_timeout: Duration::from_secs(60),
    }
}

pub fn fake_codex() -> HarnessConfig {
    fake_codex_with(&[])
}

/// The real Codex, for the ignored tests.
pub fn real_codex() -> HarnessConfig {
    HarnessConfig {
        codex: Cli {
            command: vec![lobotomy_harness::codex::locate().to_string_lossy().into_owned()],
            reasoning_effort: Some("low".into()),
        },
        interrupt_helper: vec![env!("CARGO_BIN_EXE_lobotomyd").into(), "ctrl-c".into()],
        probe_timeout: Duration::from_secs(120),
    }
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// The user's repository: one commit on `main`.
pub fn user_repo(dir: &Path) -> PathBuf {
    let repo = dir.join("repo");
    if !repo.exists() {
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        git(&repo, &["config", "user.name", "Test User"]);
        git(&repo, &["config", "user.email", "user@example.com"]);
        git(&repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("README.md"), "# project\n").unwrap();
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "initial"]);
    }
    repo
}

/// A project connected to the user's repository in `dir`; the same directory opens the same
/// project again.
pub async fn open_project(dir: &Path, harness: HarnessConfig) -> Arc<Project> {
    let host = Arc::new(Host::open(&host_dir(dir), harness).unwrap());
    let project = Arc::new(Project::open(&dir.join("project"), host).unwrap());
    if project.db.read(lobotomy_core::project::load_project).unwrap().is_none() {
        lobotomyd::onboard::onboard(&project, &user_repo(dir)).await.unwrap();
    }
    project
}

pub fn host_dir(dir: &Path) -> PathBuf {
    dir.join("host")
}

pub async fn start(dir: &Path) -> Backend {
    start_with(dir, fake_codex()).await
}

pub async fn start_with(dir: &Path, harness: HarnessConfig) -> Backend {
    Backend::start(open_project(dir, harness).await, 0).await.unwrap()
}

pub fn create_task(db: &Db, body: &str) -> String {
    db.execute(
        &Caller::User,
        &CreateTask {
            request_id: format!("c-{body}"),
            title: "测试任务".into(),
            body: body.into(),
            criteria: "work.txt 存在".into(),
            executor: "Malkuth".into(),
        },
    )
    .unwrap()
    .id
}

/// What the scheduler does when the role is free: starts the attempt and writes the slot. For
/// setting up "the last run" without a running backend.
pub async fn start_attempt(project: &Arc<Project>, task: &str) {
    project.db.execute(&Caller::Runtime, &StartAttempt { task_id: task.into(), code_start: None }).unwrap();
    for ws in project.db.read(materializing).unwrap() {
        lobotomyd::results::materialize(project.clone(), ws).await.unwrap();
    }
}

pub fn last(db: &Db) -> Option<Turn> {
    db.read(|c| last_turn(c, "Malkuth")).unwrap()
}

pub async fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub async fn ended_turn(db: &Db) -> Turn {
    wait_for("the turn to end", || last(db).filter(|t| t.state == TurnState::Ended)).await
}

pub async fn phase(db: &Db, task: &str, phase: Phase) {
    wait_for(&format!("task {task} to be {}", phase.as_str()), || {
        (db.read(|c| load_task(c, task)).unwrap().phase == phase).then_some(())
    })
    .await
}

pub fn slot(dir: &Path) -> PathBuf {
    dir.join("project").join("slots").join("worker")
}

/// Where the fake CLI leaves its diagnostics: inside `.git` in a slot, so captures skip them.
pub fn diag(dir: &Path) -> PathBuf {
    if dir.join(".git").exists() { dir.join(".git") } else { dir.to_path_buf() }
}

/// Lets the scheduler run a few rounds.
pub async fn settle(backend: &Backend) {
    backend.project.wake.notify_one();
    tokio::time::sleep(Duration::from_millis(1500)).await;
}
