//! Helpers for the backend tests with a fake Codex (tests/fixtures/fake-codex.mjs, run by Node).
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lobotomy_core::task::{CreateTask, Phase, StartAttempt, load_task};
use lobotomy_core::turn::{Turn, TurnState, last_turn};
use lobotomy_core::workspace::{AlignIdleSlot, materializing};
use lobotomy_core::{Caller, Db};
use lobotomyd::host::{Cli, HarnessConfig, Host};
use lobotomyd::project::Project;

/// A fake CLI from `tests/fixtures`, run by Node. `flags` go before the CLI's own arguments.
fn fake(script: &str, flags: &[&str]) -> Cli {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(script);
    let mut command = vec!["node".to_owned(), script.to_string_lossy().into_owned()];
    command.extend(flags.iter().map(|f| (*f).to_owned()));
    Cli { command, reasoning_effort: None }
}

/// The fake CLIs. `flags` go before Codex's own arguments, such as `--fake-hang`.
pub fn fake_codex_with(flags: &[&str]) -> HarnessConfig {
    HarnessConfig {
        claude: fake("fake-claude.mjs", &[]),
        codex: fake("fake-codex.mjs", flags),
        interrupt_helper: vec![env!("CARGO_BIN_EXE_lobotomyd").into(), "ctrl-c".into()],
        terminal_helper: env!("CARGO_BIN_EXE_lobotomyd").into(),
        probe_timeout: Duration::from_secs(60),
    }
}

pub fn fake_codex() -> HarnessConfig {
    fake_codex_with(&[])
}

/// The real CLIs, for the ignored tests.
pub fn real_clis() -> HarnessConfig {
    HarnessConfig {
        claude: Cli {
            command: vec![lobotomy_harness::claude::locate().to_string_lossy().into_owned()],
            reasoning_effort: Some("low".into()),
        },
        codex: Cli {
            command: vec![lobotomy_harness::codex::locate().to_string_lossy().into_owned()],
            reasoning_effort: Some("low".into()),
        },
        interrupt_helper: vec![env!("CARGO_BIN_EXE_lobotomyd").into(), "ctrl-c".into()],
        terminal_helper: env!("CARGO_BIN_EXE_lobotomyd").into(),
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

/// The project connected to the user's repository in `dir`, with its data in `dir/project`,
/// registered with the host in `dir/host` as the GUI's new project does. The same directory
/// opens the same project again.
pub async fn open_project(dir: &Path, harness: HarnessConfig) -> Arc<Project> {
    let host = Arc::new(Host::open(&host_dir(dir), harness).unwrap());
    let data = dir.join("project");
    match project_id(&host, &data) {
        Some(id) => Arc::new(Project::open(&id, &data, host).unwrap()),
        None => lobotomyd::projects::create(&host, &user_repo(dir), Some(data)).await.unwrap(),
    }
}

/// The project of [`open_project`] with its idle slot written, so a backend started on it has
/// nothing to do. Returns its host and id; the instance is closed.
pub async fn idle_project(dir: &Path, harness: HarnessConfig) -> (Arc<Host>, String) {
    let project = open_project(dir, harness).await;
    project.db.execute(&Caller::Runtime, &AlignIdleSlot { role: "Malkuth".into() }).unwrap();
    for ws in project.db.read(materializing).unwrap() {
        lobotomyd::results::materialize(project.clone(), ws).await.unwrap();
    }
    (project.host.clone(), project.id.clone())
}

/// The id `data` is registered under.
pub fn project_id(host: &Host, data: &Path) -> Option<String> {
    let data = data.to_string_lossy();
    host.db.projects().unwrap().into_iter().find(|p| p.data_dir == data).map(|p| p.id)
}

pub fn host_dir(dir: &Path) -> PathBuf {
    dir.join("host")
}

/// A backend running the project of [`open_project`].
pub struct Started {
    pub backend: lobotomyd::Backend,
    pub project: Arc<Project>,
    pub addr: SocketAddr,
}

impl Started {
    pub async fn shutdown(self, grace: Duration) {
        let Started { backend, project, .. } = self;
        drop(project);
        backend.shutdown(grace).await;
    }
}

pub async fn start(dir: &Path) -> Started {
    start_with(dir, fake_codex()).await
}

pub async fn start_with(dir: &Path, harness: HarnessConfig) -> Started {
    // Keep runtime errors in the failing test's captured output.
    let _ = tracing_subscriber::fmt().with_env_filter("lobotomyd=info").with_test_writer().with_ansi(false).try_init();
    let host = open_project(dir, harness).await.host.clone();
    let backend = lobotomyd::Backend::start(host, 0).await.unwrap();
    let id = project_id(&backend.host, &dir.join("project")).unwrap();
    let project = backend.project(&id).expect("the project opened");
    Started { addr: backend.addr, backend, project }
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

/// Waits until `check` gives a value, keeping the existing CI deadline and polling interval.
pub async fn wait_for<T>(what: &str, check: impl FnMut() -> Option<T>) -> T {
    wait_for_diagnostic(what, check, String::new).await
}

/// The same deadline and polling, with a snapshot only if the wait fails.
async fn wait_for_diagnostic<T>(
    what: &str,
    mut check: impl FnMut() -> Option<T>,
    diagnostic: impl FnOnce() -> String,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(value) = check() {
            return value;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for {what}\n{}", diagnostic());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Capture persisted progress and in-memory owners before the temporary project is dropped.
/// Check runs are recorded together at FinishVerification; no rows alone do not prove that
/// no check started. Tokens and raw CLI arguments are deliberately excluded.
pub fn backend_diagnostic(project: &Project) -> String {
    let persisted = project.db.read(|c| {
        let mut snapshot = serde_json::Map::new();
        for (name, sql) in [
            ("tasks", "SELECT id, phase FROM task"),
            ("turns", "SELECT id, task_id, attempt_id, state, outcome, failure, pid, process_start, done_at, registered_at, started_at, ended_at FROM turn ORDER BY registered_at, id"),
            ("captures", "SELECT id, turn_id, state, kind, detail, created_at, finished_at FROM capture"),
            ("verifications", "SELECT id, task_id, state, created_at, finished_at FROM verification"),
            ("check_runs", "SELECT verification_id, seq, exit_code, timed_out, duration_ms, output FROM check_run ORDER BY verification_id, seq"),
            ("workspaces", "SELECT name, state, target, head FROM workspace"),
            ("events", "SELECT seq, kind, entity, created_at FROM event ORDER BY seq DESC LIMIT 30"),
        ] {
            let mut stmt = c.prepare(sql)?;
            let columns: Vec<String> = stmt.column_names().iter().map(|s| (*s).into()).collect();
            let rows = stmt.query_map([], |r| {
                let mut row = serde_json::Map::new();
                for (i, column) in columns.iter().enumerate() {
                    let value = match r.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                        rusqlite::types::ValueRef::Integer(n) => n.into(),
                        rusqlite::types::ValueRef::Real(n) => serde_json::json!(n),
                        rusqlite::types::ValueRef::Text(s) => String::from_utf8_lossy(s).into_owned().into(),
                        rusqlite::types::ValueRef::Blob(_) => "<blob>".into(),
                    };
                    row.insert(column.clone(), value);
                }
                Ok(serde_json::Value::Object(row))
            })?.collect::<rusqlite::Result<Vec<_>>>()?;
            snapshot.insert(name.into(), rows.into());
        }
        Ok(serde_json::Value::Object(snapshot))
    });
    let mut jobs: Vec<_> = project.jobs.lock().unwrap().iter().cloned().collect();
    jobs.sort();
    let running = project.running.lock().unwrap().clone();
    let failed = lobotomyd::results::failures(project);
    format!(
        "project={}\njobs={jobs:?}\nrunning={running:?}\nfailed={failed:?}\n{}",
        project.data_dir.display(),
        match persisted {
            Ok(value) => serde_json::to_string_pretty(&value).unwrap(),
            Err(error) => format!("snapshot failed: {error}"),
        }
    )
}

pub async fn wait_for_backend<T>(project: &Project, what: &str, check: impl FnMut() -> Option<T>) -> T {
    wait_for_diagnostic(what, check, || backend_diagnostic(project)).await
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
pub async fn settle(backend: &Started) {
    backend.project.wake.notify_one();
    tokio::time::sleep(Duration::from_millis(1500)).await;
}
