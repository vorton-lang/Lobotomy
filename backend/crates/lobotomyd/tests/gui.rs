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

/// Each harness's permission mode is a host setting the GUI shows and changes; there is no mode
/// that asks the user (harness-adapter.md §1.9).
#[tokio::test]
async fn the_gui_sets_each_harness_permission_mode() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let mut gui = Client::connect(&backend).await;
    let snapshot = gui.call("snapshot", json!({})).await.unwrap();
    assert_eq!(
        snapshot["harnesses"],
        json!([{ "harness": "claude", "permission": "full" }, { "harness": "codex", "permission": "full" }])
    );

    // Each harness has its own setting: an environment may refuse one and not the other.
    let set = json!({ "harness": "codex", "permission": "auto_review" });
    assert_eq!(gui.command("set_permission", set.clone()).await.unwrap(), set);
    let snapshot = gui.call("snapshot", json!({})).await.unwrap();
    assert_eq!(
        snapshot["harnesses"],
        json!([{ "harness": "claude", "permission": "full" }, { "harness": "codex", "permission": "auto_review" }])
    );
    assert_eq!(
        backend.project.host.permission(lobotomy_harness::Harness::Codex).unwrap(),
        lobotomy_harness::Permission::AutoReview
    );

    // The raw output turns keep, shown in the settings (#16).
    let usage = gui.call("disk_usage", json!({})).await.unwrap();
    assert_eq!(usage["raw_output"]["files"], 0, "{usage}");

    for wrong in
        [json!({ "harness": "codex", "permission": "manual" }), json!({ "harness": "nope", "permission": "full" })]
    {
        assert!(gui.command("set_permission", wrong.clone()).await.is_err(), "{wrong}");
    }
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

/// Search finds what the thread shows, in the thread's order: messages at the start of their turn,
/// items, then queued messages.
#[tokio::test]
async fn search_finds_the_threads_text_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;
    let mut gui = Client::connect(&backend).await;
    let args = json!({ "request_id": "r1", "title": "写 work.txt", "body": "FAKE:done", "criteria": "-", "executor": "Malkuth" });
    gui.command("create_task", args).await.unwrap();
    gui.event("task.accepting").await;
    // Outside execution a message waits in the queue.
    gui.command("send_message", json!({ "request_id": "m1", "role": "Malkuth", "body": "work.txt 再改一下" }))
        .await
        .unwrap();
    let thread = gui.call("thread", json!({ "role": "Malkuth" })).await.unwrap();
    let item = |kind: &str| thread["items"].as_array().unwrap().iter().find(|i| i["kind"] == kind).unwrap().clone();
    let first_seq = thread["items"][0]["seq"].clone();
    // A turn's MCP token is for its CLI only (#16).
    let snapshot = gui.call("snapshot", json!({})).await.unwrap();
    for view in [&thread, &snapshot] {
        assert!(!view.to_string().contains("\"tok_"), "a turn token reached the GUI: {view}");
    }

    let search = |query: &str| json!({ "role": "Malkuth", "query": query });
    let found = gui.call("search", search("WORK.TXT")).await.unwrap();
    let matches = found["matches"].as_array().unwrap();
    let kinds: Vec<&str> = matches.iter().map(|m| m["kind"].as_str().unwrap()).collect();
    // The brief names the criteria file, the report's body says it wrote it, the queued message
    // mentions it.
    assert_eq!(kinds, ["message", "item", "message"], "{found}");
    assert_eq!(matches[0]["seq"], first_seq, "a message sits at its turn's first item");
    assert_eq!(matches[1]["id"], item("mcp_call")["id"], "the report is found by its recorded body");
    assert_eq!(matches[2]["seq"], Value::Null, "a queued message comes last");

    let found = gui.call("search", search("finished")).await.unwrap();
    assert_eq!(found["matches"].as_array().unwrap().len(), 1);
    assert_eq!(gui.call("search", search("  ")).await.unwrap()["matches"], json!([]));

    // A rejected command reports its code.
    let wrong = json!({ "request_id": "m2", "role": "Nobody", "body": "-" });
    assert_eq!(gui.command("send_message", wrong).await.unwrap_err()["code"], "unknown_role");
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

/// Upgraded sockets are not owned by axum's HTTP server. Shutdown must join them too,
/// including clients that never send a close frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_releases_the_project_with_connected_gui_clients() {
    use lobotomyd::{host::Host, project::Project};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::open(&host_dir(dir.path()), fake_codex()).unwrap());
    let data = dir.path().join("project");
    for _ in 0..20 {
        let backend =
            lobotomyd::Backend::start(Arc::new(Project::open(&data, host.clone()).unwrap()), 0).await.unwrap();
        let first = Client::connect(&backend).await;
        let second = Client::connect(&backend).await;
        let project = Arc::downgrade(&backend.project);
        backend.shutdown(Duration::from_secs(5)).await;
        // Keep both client sockets alive. Returning from shutdown, not closing a client or
        // retrying the lock, must establish that the backend released the project.
        assert_eq!(project.strong_count(), 0, "project owners remain after shutdown");
        drop(Project::open(&data, host.clone()).unwrap());
        drop((first, second));
    }
}

/// A request already admitted must finish even when its client disconnects. Use a gated fake
/// quota probe to observe admission and completion without relying on a fast request's timing.
#[tokio::test]
async fn shutdown_drains_an_admitted_gui_request_after_disconnect() {
    admitted_gui_request_shutdown(false).await;
}

#[tokio::test]
async fn shutdown_leaves_a_timed_out_gui_request_and_its_lock_alive() {
    admitted_gui_request_shutdown(true).await;
}

async fn admitted_gui_request_shutdown(expires: bool) {
    use lobotomyd::{host::Host, project::Project};
    use std::sync::Arc;

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
    let host = Arc::new(Host::open(&host_dir(dir.path()), harness).unwrap());
    host.db.block("codex", None, "test quota block", 1).unwrap();
    let data = dir.path().join("project");
    let backend = lobotomyd::Backend::start(Arc::new(Project::open(&data, host.clone()).unwrap()), 0).await.unwrap();
    let mut client = Client::connect(&backend).await;
    let request =
        json!({"id": 1, "method": "command", "params": {"name": "quota_retry", "args": {"harness": "codex"}}});
    client.socket.send(Message::Text(request.to_string().into())).await.unwrap();
    let probe = host.dir.join("probe");
    wait_for("the admitted quota request", || probe.join("started").exists().then_some(())).await;
    drop(client);
    let project = Arc::downgrade(&backend.project);
    let grace = if expires { Duration::from_millis(10) } else { Duration::from_secs(5) };
    let mut shutdown = tokio::spawn(backend.shutdown(grace));
    let finishing = if expires {
        tokio::time::timeout(Duration::from_secs(5), &mut shutdown).await.unwrap().unwrap();
        assert!(Project::open(&data, host.clone()).is_err());
        assert!(host.db.domain("codex").unwrap().is_blocked());
        Some(project.upgrade().expect("the admitted request owns the project"))
    } else {
        // A bounded negative assertion: the gate is the cause of waiting, not a slow process.
        assert!(tokio::time::timeout(Duration::from_millis(100), &mut shutdown).await.is_err());
        None
    };
    std::fs::write(probe.join("release"), "").unwrap();
    if let Some(project) = finishing {
        wait_for("the admitted request to release its project", || (Arc::strong_count(&project) == 1).then_some(()))
            .await;
        // Synchronize the final destructor rather than observing a zero weak count.
        drop(project);
    } else {
        tokio::time::timeout(Duration::from_secs(5), shutdown).await.unwrap().unwrap();
    }
    drop(Project::open(&data, host.clone()).unwrap());
    assert!(!host.db.domain("codex").unwrap().is_blocked());
}
