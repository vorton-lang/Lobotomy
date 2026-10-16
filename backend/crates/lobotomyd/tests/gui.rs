//! The GUI's WebSocket service against a running backend with the fake Codex (frontend.md §1, §3).

mod common;

use std::collections::VecDeque;
use std::time::Duration;

use common::*;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Pushed events not yet waited for, in the order the backend sent them.
#[derive(Default)]
struct Events(VecDeque<Value>);

impl Events {
    fn add(&mut self, push: Value) {
        if push["type"] == "events" {
            self.0.extend(push["events"].as_array().unwrap().iter().cloned());
        }
    }

    /// The next event of this kind. It and the events before it are consumed; the events after it
    /// stay for the next wait, even when they came in the same push (#13).
    fn take(&mut self, kind: &str) -> Option<Value> {
        let at = self.0.iter().position(|e| e["kind"] == kind)?;
        self.0.drain(..=at).next_back()
    }
}

struct Client {
    socket: Socket,
    next_id: u64,
    events: Events,
    /// The project its requests are about, unless they name one.
    project: String,
}

impl Client {
    async fn connect(backend: &Started) -> Client {
        Client::open(backend.addr, &backend.project.host.gui_token, &backend.project.id).await
    }

    async fn open(addr: std::net::SocketAddr, token: &str, project: &str) -> Client {
        let url = format!("ws://{addr}/gui?token={token}");
        let (socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        Client { socket, next_id: 0, events: Events::default(), project: project.to_owned() }
    }

    async fn call(&mut self, method: &str, mut params: Value) -> Result<Value, Value> {
        if method != "host" && params.get("project").is_none() {
            params["project"] = self.project.clone().into();
        }
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "id": id, "method": method, "params": params });
        self.socket.send(Message::Text(request.to_string().into())).await.unwrap();
        loop {
            let message =
                tokio::time::timeout(Duration::from_secs(30), self.socket.next()).await.unwrap().unwrap().unwrap();
            let Message::Text(text) = message else { continue };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["id"] == json!(id) {
                return match value.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(value["result"].clone()),
                };
            }
            self.events.add(value);
        }
    }

    async fn command(&mut self, name: &str, args: Value) -> Result<Value, Value> {
        self.call("command", json!({ "name": name, "args": args })).await
    }

    /// Waits for the next pushed event of this kind.
    async fn event(&mut self, kind: &str) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(event) = self.events.take(kind) {
                return event;
            }
            let message =
                tokio::time::timeout_at(deadline, self.socket.next()).await.expect("no such event").unwrap().unwrap();
            if let Message::Text(text) = message {
                self.events.add(serde_json::from_str(&text).unwrap());
            }
        }
    }
}

#[test]
fn waiting_for_an_event_keeps_the_later_events_of_its_push() {
    let mut events = Events::default();
    let kinds = ["turn.registered", "turn.ended", "task.accepting", "task.done"];
    events.add(json!({ "type": "events", "events": kinds.map(|kind| json!({ "kind": kind })) }));
    assert!(events.take("turn.ended").is_some());
    assert!(events.take("task.done").is_some());
    // What came before the awaited events was consumed with them.
    assert!(events.take("turn.registered").is_none());
}

/// A turn that ends without done or a question leaves the task to the user, with the refused
/// report as the reason when there is one (#13).
#[tokio::test]
async fn a_turn_without_a_report_leaves_the_task_to_the_user() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let mut gui = Client::connect(&backend).await;
    let args = json!({ "request_id": "r1", "title": "没有报告", "body": "FAKE:refused", "criteria": "-", "executor": "Malkuth" });
    let task = gui.command("create_task", args).await.unwrap()["id"].as_str().unwrap().to_owned();
    gui.event("turn.ended").await;
    let snapshot = gui.call("snapshot", json!({})).await.unwrap();
    let attention = snapshot["attention"].as_array().unwrap();
    assert_eq!(attention.len(), 1, "{attention:?}");
    assert_eq!(
        (attention[0]["kind"].as_str(), attention[0]["task_id"].as_str()),
        (Some("stalled"), Some(task.as_str()))
    );
    assert!(attention[0]["report_error"].as_str().unwrap().contains("requires approval"), "{attention:?}");

    // A message moves it on. This turn keeps running, so the snapshot cannot race its end.
    let message = json!({ "request_id": "m1", "role": "Malkuth", "task_id": task, "body": "FAKE:sleep" });
    gui.command("send_message", message).await.unwrap();
    gui.event("turn.running").await;
    let snapshot = gui.call("snapshot", json!({})).await.unwrap();
    assert_eq!(snapshot["attention"], json!([]));
    backend.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn the_gui_can_stop_the_backend() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let mut gui = Client::connect(&backend).await;
    let host = backend.project.host.clone();
    // Like main: start shutting down as soon as the command is accepted, without waiting
    // for the client's reply. Waiting for command() first would conceal a dropped reply.
    let stopped = tokio::spawn(async move {
        host.shutdown_requested.notified().await;
        backend.shutdown(Duration::from_secs(5)).await;
    });
    gui.command("shutdown", json!({})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), stopped).await.unwrap().unwrap();
}

/// Upgraded sockets are not owned by axum's HTTP server. Shutdown must join them too,
/// including clients that never send a close frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_releases_the_project_with_connected_gui_clients() {
    use lobotomyd::project::Project;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let (host, id) = idle_project(dir.path(), fake_codex()).await;
    let data = dir.path().join("project");
    for _ in 0..20 {
        let backend = lobotomyd::Backend::start(host.clone(), 0).await.unwrap();
        let mut first = Client::open(backend.addr, &host.gui_token, &id).await;
        let second = Client::open(backend.addr, &host.gui_token, &id).await;
        // A request about the project went through the connection.
        first.call("snapshot", json!({})).await.unwrap();
        let project = Arc::downgrade(&backend.project(&id).unwrap());
        backend.shutdown(Duration::from_secs(5)).await;
        // Keep both client sockets alive. Returning from shutdown, not closing a client or
        // retrying the lock, must establish that the backend released the project.
        assert_eq!(project.strong_count(), 0, "project owners remain after shutdown");
        drop(Project::open(&id, &data, host.clone()).unwrap());
        drop((first, second));
    }
}

/// A request already admitted must finish even when its client disconnects. Use a gated fake
/// quota probe to observe admission and completion without relying on a fast request's timing.
#[tokio::test]
async fn shutdown_drains_an_admitted_gui_request_after_disconnect() {
    admitted_gui_request_shutdown(false).await;
}

/// Shutdown does not wait past its grace period for a request; the request still finishes.
#[tokio::test]
async fn shutdown_leaves_a_timed_out_gui_request_running() {
    admitted_gui_request_shutdown(true).await;
}

async fn admitted_gui_request_shutdown(expires: bool) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("gated-probe.mjs");
    std::fs::write(
        &script,
        r#"
        import fs from 'node:fs';
        for await (const chunk of process.stdin) {}
        fs.writeFileSync('started', '');
        while (!fs.existsSync('release')) await new Promise(resolve => setTimeout(resolve, 10));
        console.log(JSON.stringify({type: 'turn.completed', usage: {input_tokens: 1, output_tokens: 1}}));
    "#,
    )
    .unwrap();
    let mut harness = fake_codex();
    harness.codex.command = vec!["node".into(), script.to_string_lossy().into_owned()];
    let (host, id) = idle_project(dir.path(), harness).await;
    host.db.block("codex", None, "test quota block", 1).unwrap();
    let backend = lobotomyd::Backend::start(host.clone(), 0).await.unwrap();
    let mut client = Client::open(backend.addr, &host.gui_token, &id).await;
    // A host command: the domain belongs to no project.
    let request =
        json!({"id": 1, "method": "command", "params": {"name": "quota_retry", "args": {"harness": "codex"}}});
    client.socket.send(Message::Text(request.to_string().into())).await.unwrap();
    let probe = host.dir.join("probe");
    wait_for("the admitted quota request", || probe.join("started").exists().then_some(())).await;
    drop(client);
    let grace = if expires { Duration::from_millis(10) } else { Duration::from_secs(5) };
    let mut shutdown = tokio::spawn(backend.shutdown(grace));
    if expires {
        tokio::time::timeout(Duration::from_secs(5), &mut shutdown).await.unwrap().unwrap();
        assert!(host.db.domain("codex").unwrap().is_blocked(), "the check has not finished");
    } else {
        // A bounded negative assertion: the gate is the cause of waiting, not a slow process.
        assert!(tokio::time::timeout(Duration::from_millis(100), &mut shutdown).await.is_err());
    }
    std::fs::write(probe.join("release"), "").unwrap();
    if !expires {
        tokio::time::timeout(Duration::from_secs(5), shutdown).await.unwrap().unwrap();
    }
    // The request finishes and records the check, though its client left.
    wait_for("the check to be recorded", || (!host.db.domain("codex").unwrap().is_blocked()).then_some(())).await;
}
