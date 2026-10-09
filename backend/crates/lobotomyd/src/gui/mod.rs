//! The GUI's connection: one WebSocket per window (frontend.md §1, §3).
//!
//! - The client sends requests `{id, method, params}` and gets `{id, result}` or
//!   `{id, error: {code, message}}`. Commands carry the GUI's request id as their idempotency
//!   key, so a repeated request does nothing twice.
//! - The server pushes what changed, not the content: `{type: "events", events}` with the global
//!   event log, `{type: "thread", role, seq}` when a thread has a new item, `{type: "live", live}`
//!   with items of running turns, `{type: "host"}` when a host setting changed, `{type: "tick"}`
//!   every 15 seconds, and `{type: "resync"}` when the client fell behind and must take a new
//!   snapshot.
//! - Error codes: a refusal's own code; `bad_request` for parameters that do not parse;
//!   `internal` for anything else.
//! - A connection needs the host's GUI token, and `Host` and `Origin` must be local.
//!
//! This module holds the connection and the pushes; `view` answers the reads, whose models are
//! in `lobotomy_core::view`, and `command` runs the commands (#16).

mod command;
mod view;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures::{SinkExt as _, StreamExt as _};
use lobotomy_core::Error;
use lobotomy_core::view::{events_after, last_event, thread_heads};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::project::Project;
use crate::runner::db;

/// How often the event log and the live view are looked at for changes.
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
const TICK: Duration = Duration::from_secs(15);
/// Pushes kept for slow clients. A client further behind gets `resync`.
pub const PUSH_BUFFER: usize = 512;

pub fn gui_router(project: Arc<Project>) -> Router {
    Router::new().route("/gui", get(upgrade)).with_state(project)
}

#[derive(Deserialize)]
struct Connect {
    token: Option<String>,
}

async fn upgrade(
    State(project): State<Arc<Project>>,
    Query(connect): Query<Connect>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !local_host(&headers) || !local_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !connect.token.as_deref().is_some_and(|t| same(t, &project.host.gui_token)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| session(socket, project))
}

/// Compares without stopping at the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn is_local(host: &str) -> bool {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    };
    matches!(name, "127.0.0.1" | "localhost" | "[::1]")
}

/// Against DNS rebinding: the request must name a loopback host.
fn local_host(headers: &HeaderMap) -> bool {
    headers.get(header::HOST).and_then(|h| h.to_str().ok()).is_some_and(is_local)
}

/// Web pages can open WebSockets to any address; only the app, files and local dev servers may.
fn local_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    if origin == "file://" {
        return true;
    }
    origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://")).is_some_and(is_local)
}

async fn session(socket: WebSocket, project: Arc<Project>) {
    let (mut sink, mut stream) = socket.split();
    let (out, mut outbox) = mpsc::channel::<String>(256);
    let writer = tokio::spawn(async move {
        while let Some(text) = outbox.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    let mut pushes = project.gui_push.subscribe();
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let (project, out) = (project.clone(), out.clone());
                    // A request may take long (a quota check); others go on meanwhile.
                    tokio::spawn(async move {
                        let reply = respond(&project, text.as_str()).await;
                        let _ = out.send(reply).await;
                    });
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            push = pushes.recv() => {
                let text = match push {
                    Ok(text) => text.to_string(),
                    Err(broadcast::error::RecvError::Lagged(_)) => json!({ "type": "resync" }).to_string(),
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                if out.send(text).await.is_err() {
                    break;
                }
            }
            _ = tick.tick() => {
                if out.send(json!({ "type": "tick" }).to_string()).await.is_err() {
                    break;
                }
            }
        }
    }
    drop(out);
    let _ = writer.await;
}

/// Pushes changes to every connected GUI: new entries of the event log and the live view of
/// running turns. Stops with `shutdown`. It reads on the read connection, so polling never waits
/// for a command, nor holds one up.
pub async fn watch(project: Arc<Project>, shutdown: CancellationToken) {
    let mut last_seq = db(&project, |db| db.read(last_event)).await.unwrap_or(0);
    let mut last_live = project.live_version.load(Ordering::Relaxed);
    let mut last_items: HashMap<String, i64> = db(&project, |db| db.read(thread_heads)).await.unwrap_or_default();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(WATCH_INTERVAL) => {}
            _ = shutdown.cancelled() => return,
        }
        let after = last_seq;
        match db(&project, move |db| db.read(|c| events_after(c, after))).await {
            Ok(events) if !events.is_empty() => {
                last_seq = events.last().map_or(last_seq, |e| e.seq);
                let _ = project.gui_push.send(json!({ "type": "events", "events": events }).to_string().into());
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = format!("{e:#}"), "could not read the event log"),
        }
        // Transcript items are not business events; each thread's newest item tells the GUI to
        // fetch what it has not seen.
        if let Ok(heads) = db(&project, |db| db.read(thread_heads)).await {
            for (role, seq) in &heads {
                if last_items.get(role) != Some(seq) {
                    let _ =
                        project.gui_push.send(json!({ "type": "thread", "role": role, "seq": seq }).to_string().into());
                }
            }
            last_items = heads;
        }
        let version = project.live_version.load(Ordering::Relaxed);
        if version != last_live {
            last_live = version;
            let live = project.live.lock().unwrap().clone();
            let _ = project.gui_push.send(json!({ "type": "live", "live": live }).to_string().into());
        }
    }
}

#[derive(Deserialize)]
struct Request {
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

async fn respond(project: &Arc<Project>, text: &str) -> String {
    let request: Request = match serde_json::from_str(text) {
        Ok(request) => request,
        Err(e) => {
            return json!({ "id": null, "error": { "code": "bad_request", "message": e.to_string() } }).to_string();
        }
    };
    match call(project, &request.method, request.params).await {
        Ok(result) => json!({ "id": request.id, "result": result }).to_string(),
        Err(e) => {
            json!({ "id": request.id, "error": { "code": error_code(&e), "message": format!("{e:#}") } }).to_string()
        }
    }
}

/// A refusal carries its code; parameters that do not parse are the client's mistake; anything
/// else is the backend's (#16).
fn error_code(e: &anyhow::Error) -> &'static str {
    if let Some(e) = e.downcast_ref::<Error>() {
        return e.code().unwrap_or("internal");
    }
    if e.downcast_ref::<serde_json::Error>().is_some() { "bad_request" } else { "internal" }
}

/// `command` changes something; every other method reads.
async fn call(project: &Arc<Project>, method: &str, params: Value) -> anyhow::Result<Value> {
    if method != "command" {
        return view::answer(project, method, params).await;
    }
    let result = command::run(project, serde_json::from_value(params)?).await?;
    project.wake.notify_one();
    Ok(result)
}
