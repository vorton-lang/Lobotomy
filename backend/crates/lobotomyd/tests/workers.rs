//! The backend gives a worker's tasks to its worker host and keeps what the worker reports
//! (tech-plan.md §7 step 1, group 7). In a file of its own: the test changes the process
//! environment.

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use futures::{SinkExt as _, StreamExt as _};
use lobotomy_core::Caller;
use lobotomy_core::task::{CreateTask, Phase, SendMessage, load_task};
use lobotomy_core::worker::worker_task;
use lobotomy_worker_protocol::event::{ArtifactKind, AttemptState};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

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

    let reply_to = db
        .execute(
            &Caller::User,
            &SendMessage {
                request_id: "reference".into(),
                role: "w".into(),
                task_id: None,
                body: "补充要求的依据".into(),
                reply_to: None,
            },
        )
        .unwrap()
        .id;
    let message = json!({
        "request_id": "retry-message", "role": "w", "task_id": task,
        "body": "FAKE:done", "reply_to": reply_to,
    });
    let first = command(&backend, "send_message", message.clone()).await.unwrap();
    let delivered_again = wait_for_delivery(&backend, &task, &report.version).await;
    assert_eq!(command(&backend, "send_message", message.clone()).await.unwrap(), first);
    let saved: (String, Option<String>) = db
        .read(|c| {
            Ok(c.query_row("SELECT role, reply_to FROM message WHERE id = ?1", [first["id"].as_str().unwrap()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?)
        })
        .unwrap();
    assert_eq!(saved, ("w".into(), Some(reply_to)));
    let mut wrong_role = message.clone();
    wrong_role["request_id"] = "wrong-role".into();
    wrong_role["role"] = "Angela".into();
    assert_eq!(command(&backend, "send_message", wrong_role).await.unwrap_err()["code"], "wrong_executor");
    let mut wrong_reply = message;
    wrong_reply["request_id"] = "wrong-reply".into();
    wrong_reply["reply_to"] = "missing".into();
    assert_eq!(command(&backend, "send_message", wrong_reply).await.unwrap_err()["code"], "not_found");

    let report = delivered_again.artifacts.iter().find(|a| a.kind == ArtifactKind::Report).unwrap();
    let back = json!({ "request_id": "retry-send-back", "task_id": task, "reason": "FAKE:done" });
    command(&backend, "send_back", back.clone()).await.unwrap();
    wait_for_delivery(&backend, &task, &report.version).await;
    command(&backend, "send_back", back).await.unwrap();
    let project_messages: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM message WHERE task_id = ?1", [&task], |r| r.get(0))?))
        .unwrap();
    assert_eq!(project_messages, 2, "retries must reuse the project's messages");
    let worker_root = host_dir(dir.path()).join("workers/w");
    let worker_db = rusqlite::Connection::open(worker_root.join("worker.db")).unwrap();
    let forwarded: i64 = worker_db.query_row("SELECT count(*) FROM message", [], |r| r.get(0)).unwrap();
    assert_eq!(forwarded, 2, "each GUI submission reaches the worker once");
    let agent_db = rusqlite::Connection::open(worker_root.join("tasks").join(&task).join("lobotomy.db")).unwrap();
    let turns: i64 = agent_db.query_row("SELECT count(*) FROM turn", [], |r| r.get(0)).unwrap();
    assert_eq!(turns, 3, "one initial turn and one per distinct GUI submission");
    backend.shutdown(Duration::from_secs(15)).await;
}

async fn wait_for_delivery(backend: &Started, task: &str, after: &str) -> lobotomy_core::worker::WorkerTask {
    wait_for_backend(&backend.project, "the worker's next delivery", || {
        backend.project.db.read(|c| worker_task(c, task)).unwrap().filter(|w| {
            w.status.state == AttemptState::Delivered
                && w.artifacts.iter().any(|a| a.kind == ArtifactKind::Report && a.version != after)
        })
    })
    .await
}

async fn command(backend: &Started, name: &str, args: Value) -> Result<Value, Value> {
    let url = format!("ws://{}/gui?token={}", backend.addr, backend.project.host.gui_token);
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let request = json!({
        "id": 1, "method": "command",
        "params": { "project": backend.project.id, "name": name, "args": args },
    });
    socket.send(Message::Text(request.to_string().into())).await.unwrap();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), socket.next()).await.unwrap().unwrap().unwrap();
        let Message::Text(text) = message else { continue };
        let value: Value = serde_json::from_str(&text).unwrap();
        if value["id"] == 1 {
            return match value.get("error") {
                Some(error) => Err(error.clone()),
                None => Ok(value["result"].clone()),
            };
        }
    }
}
