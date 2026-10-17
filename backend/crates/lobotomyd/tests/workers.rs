//! The backend gives a worker's tasks to its worker host and keeps what the worker reports
//! (tech-plan.md §7 step 1, group 7). In a file of its own: the test changes the process
//! environment.

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use lobotomy_core::Caller;
use lobotomy_core::task::{CreateTask, Phase, load_task};
use lobotomy_core::worker::worker_task;
use lobotomy_worker_protocol::event::{ArtifactKind, AttemptState};

#[tokio::test]
async fn a_task_for_a_worker_goes_to_its_worker_host_and_comes_back_delivered() {
    let program = Path::new(env!("CARGO_BIN_EXE_lobotomyd"))
        .with_file_name(format!("lobotomy-worker{}", std::env::consts::EXE_SUFFIX));
    assert!(program.exists(), "build the worker host first: cargo build -p lobotomy-worker");
    let codex = serde_json::to_string(&fake_codex().codex.command).unwrap();
    // SAFETY: the only test in this binary, set before any thread reads the environment.
    unsafe {
        std::env::set_var("LOBOTOMY_WORKER", &program);
        std::env::set_var("LOBOTOMY_CODEX", codex);
    }
    let dir = tempfile::tempdir().unwrap();
    let (host, id) = idle_project(dir.path(), fake_codex()).await;
    let name = host.db.project(&id).unwrap().unwrap().name;
    let config = format!("[worker.w]\nproject = \"{name}\"\nagent = {{ harness = \"codex\" }}\n");
    std::fs::write(host_dir(dir.path()).join("workers.toml"), config).unwrap();
    drop(host);

    let backend = start(dir.path()).await;
    let create = CreateTask {
        request_id: "t".into(),
        title: "写 work.txt".into(),
        body: "FAKE:done".into(),
        criteria: "work.txt 存在".into(),
        executor: "w".into(),
    };
    let task = backend.project.db.execute(&Caller::User, &create).unwrap().id;
    backend.project.wake.notify_one();
    let db = &backend.project.db;
    let delivered = wait_for_backend(&backend.project, "the worker to deliver", || {
        db.read(|c| worker_task(c, &task)).unwrap().filter(|w| w.status.state == AttemptState::Delivered)
    })
    .await;
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().phase, Phase::Accepting);
    let report = delivered.artifacts.iter().find(|a| a.kind == ArtifactKind::Report).expect("the agent's account");
    assert!(report.text.as_deref().unwrap().contains("写了 work.txt"));
    backend.shutdown(Duration::from_secs(15)).await;
}
