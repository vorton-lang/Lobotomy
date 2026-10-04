//! Helpers for the backend tests with a fake Codex (tests/fixtures/fake-codex.mjs, run by Node).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lobotomy_core::task::CreateTask;
use lobotomy_core::turn::{Turn, TurnState, last_turn};
use lobotomy_core::{Caller, Db};
use lobotomyd::Backend;
use lobotomyd::host::{HarnessConfig, Host};
use lobotomyd::project::Project;

/// The fake Codex. `flags` go before Codex's own arguments, such as `--fake-hang`.
pub fn fake_codex_with(flags: &[&str]) -> HarnessConfig {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("fake-codex.mjs");
    let mut codex = vec!["node".to_owned(), script.to_string_lossy().into_owned()];
    codex.extend(flags.iter().map(|f| (*f).to_owned()));
    HarnessConfig {
        codex,
        interrupt_helper: vec![env!("CARGO_BIN_EXE_lobotomyd").into(), "ctrl-c".into()],
        codex_reasoning_effort: None,
        probe_timeout: Duration::from_secs(60),
    }
}

pub fn fake_codex() -> HarnessConfig {
    fake_codex_with(&[])
}

/// The real Codex, for the ignored tests.
pub fn real_codex() -> HarnessConfig {
    HarnessConfig {
        codex: vec![lobotomy_harness::codex::locate().to_string_lossy().into_owned()],
        interrupt_helper: vec![env!("CARGO_BIN_EXE_lobotomyd").into(), "ctrl-c".into()],
        codex_reasoning_effort: Some("low".into()),
        probe_timeout: Duration::from_secs(120),
    }
}

pub fn open_project(dir: &Path, harness: HarnessConfig) -> Project {
    let host = Arc::new(Host::open(&host_dir(dir), harness).unwrap());
    Project::open(&dir.join("project"), host).unwrap()
}

pub fn host_dir(dir: &Path) -> PathBuf {
    dir.join("host")
}

pub async fn start(dir: &Path) -> Backend {
    Backend::start(Arc::new(open_project(dir, fake_codex())), 0).await.unwrap()
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

pub fn slot(dir: &Path) -> PathBuf {
    dir.join("project").join("slots").join("worker")
}

/// Lets the scheduler run a few rounds.
pub async fn settle(backend: &Backend) {
    backend.project.wake.notify_one();
    tokio::time::sleep(Duration::from_millis(1500)).await;
}
