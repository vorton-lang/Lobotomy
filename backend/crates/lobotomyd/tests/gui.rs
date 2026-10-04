//! The GUI's WebSocket service against a running backend with the fake Codex (frontend.md §1, §3).

mod common;

use std::collections::VecDeque;
use std::time::Duration;

use common::*;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

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
}

impl Client {
    async fn connect(backend: &lobotomyd::Backend) -> Client {
        let url = format!("ws://{}/gui?token={}", backend.addr, backend.project.host.gui_token);
        let (socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        Client { socket, next_id: 0, events: Events::default() }
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "id": id, "method": method, "params": params });
        self.socket.send(Message::Text(request.to_string().into())).await.unwrap();
        loop {
            let message = tokio::time::timeout(Duration::from_secs(30), self.socket.next()).await.unwrap().unwrap().unwrap();
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
            let message = tokio::time::timeout_at(deadline, self.socket.next()).await.expect("no such event").unwrap().unwrap();
            if let Message::Text(text) = message {
                self.events.add(serde_json::from_str(&text).unwrap());
            }
        }
    }
}

#[test]
fn waiting_for_an_event_keeps_the_later_events_of_its_push() {
    let mut events = Events::default();
    let kinds = ["turn.registered", "turn.ended", "capture.intent", "capture.finished"];
    events.add(json!({ "type": "events", "events": kinds.map(|kind| json!({ "kind": kind })) }));
    assert!(events.take("turn.ended").is_some());
    assert!(events.take("capture.finished").is_some());
    // What came before the awaited events was consumed with them.
    assert!(events.take("turn.registered").is_none());
}

#[tokio::test]
async fn a_connection_needs_the_token_and_a_local_origin() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let status = |result: Result<_, tokio_tungstenite::tungstenite::Error>| match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => response.status().as_u16(),
        Err(other) => panic!("unexpected error {other}"),
        Ok(_) => 101,
    };
    let without = tokio_tungstenite::connect_async(format!("ws://{}/gui", backend.addr)).await;
    assert_eq!(status(without), 401);
    let wrong = tokio_tungstenite::connect_async(format!("ws://{}/gui?token=nope", backend.addr)).await;
    assert_eq!(status(wrong), 401);

    let url = format!("ws://{}/gui?token={}", backend.addr, backend.project.host.gui_token);
    let mut request = url.clone().into_client_request().unwrap();
    request.headers_mut().insert("Origin", HeaderValue::from_static("https://example.com"));
    assert_eq!(status(tokio_tungstenite::connect_async(request).await), 403);
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert("Origin", HeaderValue::from_static("http://localhost:5173"));
    assert_eq!(status(tokio_tungstenite::connect_async(request).await), 101);
    backend.shutdown(Duration::from_secs(5)).await;
}

/// The M1 chain through the GUI's interface: create a task, watch it run, look at the
/// candidate's diff, accept it.
#[tokio::test]
async fn the_gui_drives_a_task_to_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let mut gui = Client::connect(&backend).await;

    let snapshot = gui.call("snapshot", json!({})).await.unwrap();
    assert_eq!(snapshot["project"]["branch"], "main");
    assert_eq!(snapshot["roles"][0]["name"], "Malkuth");
    assert_eq!(snapshot["attention"], json!([]));

    let args = json!({ "request_id": "r1", "title": "写 work.txt", "body": "FAKE:done", "criteria": "文件存在", "executor": "Malkuth" });
    let task = gui.command("create_task", args.clone()).await.unwrap()["id"].as_str().unwrap().to_owned();
    // The same request again changes nothing.
    assert_eq!(gui.command("create_task", args).await.unwrap()["id"], task.as_str());

    gui.event("task.accepting").await;
    let snapshot = gui.call("snapshot", json!({})).await.unwrap();
    let attention = snapshot["attention"].as_array().unwrap();
    assert_eq!(attention.len(), 1, "{attention:?}");
    assert_eq!(attention[0]["kind"], "accept");
    let verification = attention[0]["verification_id"].as_str().unwrap().to_owned();

    // The thread shows the user's brief, Malkuth's items and its done report.
    let thread = gui.call("thread", json!({ "role": "Malkuth" })).await.unwrap();
    let kinds: Vec<&str> = thread["items"].as_array().unwrap().iter().map(|i| i["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["input", "agent_message", "mcp_call", "agent_message"]);
    let call = &thread["items"][2];
    assert_eq!(thread["commands"][call["command_id"].as_str().unwrap()]["args"]["status"], "done");
    assert!(thread["messages"][0]["body"].as_str().unwrap().contains("写 work.txt"));

    // The candidate's diff against the integration version.
    let detail = gui.call("task", json!({ "task_id": task })).await.unwrap();
    let v = &detail["verifications"][0];
    assert_eq!(v["state"], "passed");
    let diff = gui.call("diff", json!({ "from": v["base"], "to": v["commit_id"] })).await.unwrap();
    assert_eq!(diff, json!([{ "path": "work.txt", "old": null, "new": { "kind": "text", "text": "hi" } }]));

    let accept = json!({
        "request_id": "a1",
        "task_id": task,
        "verification_id": verification,
        "criteria_version": 1,
        "expected_integration": snapshot["project"]["integration"],
    });
    gui.command("accept", accept).await.unwrap();
    gui.event("preview.written").await;
    let repo = user_repo(dir.path());
    assert_eq!(std::fs::read_to_string(repo.join("work.txt")).unwrap(), "hi");

    // A rejected command reports its code.
    let again = json!({ "request_id": "a2", "task_id": task, "verification_id": verification, "criteria_version": 1, "expected_integration": "x" });
    assert_eq!(gui.command("accept", again).await.unwrap_err()["code"], "not_accepting");
    backend.shutdown(Duration::from_secs(5)).await;
}

/// What stops a role shows up as something for the user to decide, and the user's command
/// resolves it.
#[tokio::test]
async fn a_failed_turn_waits_in_attention_until_the_user_continues() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let mut gui = Client::connect(&backend).await;
    let args = json!({ "request_id": "r1", "title": "失败", "body": "FAKE:fail", "criteria": "-", "executor": "Malkuth" });
    gui.command("create_task", args).await.unwrap();
    gui.event("turn.ended").await;
    // Nothing to decide until the turn's scene is captured.
    gui.event("capture.finished").await;
    let snapshot = gui.call("snapshot", json!({})).await.unwrap();
    let hold = &snapshot["attention"][0];
    assert_eq!((hold["kind"].as_str(), hold["hold"]["kind"].as_str()), (Some("hold"), Some("abnormal")));
    assert_eq!(hold["hold"]["turn"]["outcome"], "failed");

    gui.command("continue", json!({ "request_id": "k1", "role": "Malkuth" })).await.unwrap();
    gui.event("turn.registered").await;
    let snapshot = gui.call("snapshot", json!({})).await.unwrap();
    assert!(snapshot["attention"].as_array().unwrap().iter().all(|a| a["kind"] != "hold"));
    backend.shutdown(Duration::from_secs(5)).await;
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
    gui.event("capture.finished").await;
    let snapshot = gui.call("snapshot", json!({})).await.unwrap();
    let attention = snapshot["attention"].as_array().unwrap();
    assert_eq!(attention.len(), 1, "{attention:?}");
    assert_eq!((attention[0]["kind"].as_str(), attention[0]["task_id"].as_str()), (Some("stalled"), Some(task.as_str())));
    assert!(attention[0]["report_error"].as_str().unwrap().contains("requires approval"), "{attention:?}");

    // A message moves it on. This turn keeps running, so the snapshot cannot race its end.
    let message = json!({ "request_id": "m1", "role": "Malkuth", "task_id": task, "body": "FAKE:sleep" });
    gui.command("send_message", message).await.unwrap();
    gui.event("turn.running").await;
    let snapshot = gui.call("snapshot", json!({})).await.unwrap();
    assert_eq!(snapshot["attention"], json!([]));
    backend.shutdown(Duration::from_secs(5)).await;
}

/// A client that falls further behind than the push buffer is told to take a new snapshot rather
/// than silently missing pushes (frontend.md §3).
#[tokio::test]
async fn a_client_that_falls_behind_is_told_to_resync() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let mut gui = Client::connect(&backend).await;
    gui.call("snapshot", json!({})).await.unwrap();
    // The test's runtime has one thread: the session cannot forward any of these until the test
    // awaits, so it falls behind by more than the buffer holds.
    for seq in 0..lobotomyd::gui::PUSH_BUFFER + 100 {
        let push = json!({ "type": "thread", "role": "Malkuth", "seq": seq }).to_string();
        let _ = backend.project.gui_push.send(push.into());
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let message = tokio::time::timeout_at(deadline, gui.socket.next()).await.expect("no resync").unwrap().unwrap();
        let Message::Text(text) = message else { continue };
        if serde_json::from_str::<Value>(&text).unwrap()["type"] == "resync" {
            break;
        }
    }
    backend.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn the_gui_can_stop_the_backend() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let mut gui = Client::connect(&backend).await;
    let project = backend.project.clone();
    let stopped = tokio::spawn(async move { project.shutdown_requested.notified().await });
    tokio::task::yield_now().await;
    gui.command("shutdown", json!({})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), stopped).await.unwrap().unwrap();
    backend.shutdown(Duration::from_secs(5)).await;
}
