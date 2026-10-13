//! The GUI's connection: one WebSocket per window, for all projects (frontend.md §1, §3).
//!
//! - The client sends requests `{id, method, params}` and gets `{id, result}` or
//!   `{id, error: {code, message}}`. Commands carry the GUI's request id as their idempotency
//!   key, so a repeated request does nothing twice.
//! - A request about a project names it: `params.project` for reads, and for `command`, whose
//!   params are `{name, args, project}`. A request about Angela names `angela` there instead
//!   (data-model.md §10.6): her snapshot, thread, searches and blobs, and the commands about her
//!   turns and messages. `host` reads the host snapshot; the host's commands (`create_project`,
//!   `archive_project`, `unarchive_project`, `retry_open`, `set_permission`, `quota_retry`,
//!   `shutdown`) need no project.
//! - The server pushes what changed, not the content. About a project, with its id in `project`
//!   (`angela` for Angela):
//!   `{type: "events", events}` with its event log, `{type: "thread", role, seq}` when a thread
//!   has a new item, `{type: "live", live}` with items of running turns. About the host:
//!   `{type: "host"}` when a host setting or the project list changed, `{type: "tick"}` every 15
//!   seconds, and `{type: "resync"}` when the client fell behind and must take new snapshots.
//! - Error codes: a refusal's own code; `bad_request` for parameters that do not parse;
//!   `internal` for anything else.
//! - A connection needs the host's GUI token, and `Host` and `Origin` must be local.
//!
//! This module holds the connection and the pushes; `view` answers the reads, whose models are
//! in `lobotomy_core::view`, and `command` runs the commands (#16).

#[cfg(feature = "ts-rs")]
pub mod bindings;
mod command;
pub mod protocol;
mod view;

use protocol::{Push, RemoteError};
pub use view::project_attention;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message, WebSocketUpgrade};
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
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::host::Host;
use crate::instance::Instance;
use crate::project::Project;
use crate::projects::Projects;
use crate::runner::db;

/// How often the event log and the live view are looked at for changes.
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
const TICK: Duration = Duration::from_secs(15);
/// Pushes kept for slow clients. A client further behind gets `resync`.
pub const PUSH_BUFFER: usize = 512;

#[derive(Clone)]
struct Service {
    host: Arc<Host>,
    projects: Arc<Projects>,
    /// Angela's instance, or why it could not be opened.
    angela: Result<Arc<Instance>, String>,
    shutdown: CancellationToken,
    sessions: TaskTracker,
}

/// What a request is about.
enum Scope {
    Project(Arc<Project>),
    Angela(Arc<Instance>),
}

impl Service {
    /// The open project, or Angela, that a request names.
    fn scope(&self, id: Option<&str>) -> anyhow::Result<Scope> {
        let Some(id) = id else {
            return Err(Error::rejected("bad_request", "the request names no project").into());
        };
        if id == crate::angela::ID {
            return match &self.angela {
                Ok(angela) => Ok(Scope::Angela(angela.clone())),
                Err(error) => Err(Error::rejected("angela_unavailable", format!("Angela 没有打开：{error}")).into()),
            };
        }
        let project = self.projects.get(id);
        project
            .map(Scope::Project)
            .ok_or_else(|| Error::rejected("unknown_project", format!("项目 {id} 没有打开")).into())
    }
}

pub fn gui_router(
    host: Arc<Host>,
    projects: Arc<Projects>,
    angela: Result<Arc<Instance>, String>,
    shutdown: CancellationToken,
    sessions: TaskTracker,
) -> Router {
    Router::new().route("/gui", get(upgrade)).with_state(Service { host, projects, angela, shutdown, sessions })
}

#[derive(Deserialize)]
struct Connect {
    token: Option<String>,
}

async fn upgrade(
    State(service): State<Service>,
    Query(connect): Query<Connect>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if service.shutdown.is_cancelled() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if !local_host(&headers) || !local_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !connect.token.as_deref().is_some_and(|t| same(t, &service.host.gui_token)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // Register before the upgrade callback runs: shutdown can race with the handshake.
    let owner = service.sessions.token();
    ws.on_upgrade(move |socket| async move {
        let (sink, stream) = socket.split();
        session(sink, stream, service).await;
        drop(owner);
    })
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

async fn session<S, R>(mut sink: S, mut stream: R, service: Service)
where
    S: futures::Sink<Message> + Unpin,
    R: futures::Stream<Item = Result<Message, axum::Error>> + Unpin,
{
    let shutdown = service.shutdown.clone();
    let (out, mut outbox) = mpsc::channel::<String>(256);
    let mut writer = Box::pin(async move {
        while let Some(text) = outbox.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    let mut requests = JoinSet::new();
    let mut pushes = service.host.gui_push.subscribe();
    let mut tick = tokio::time::interval(TICK);
    let read = async {
        loop {
            tokio::select! {
                _ = requests.join_next(), if !requests.is_empty() => {}
                incoming = stream.next() => match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let (service, out) = (service.clone(), out.clone());
                        // A request may take long (a quota check); others go on meanwhile.
                        requests.spawn(async move {
                            let reply = respond(&service, text.as_str()).await;
                            let _ = out.send(reply).await;
                        });
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                },
                push = pushes.recv() => {
                    let text = match push {
                        Ok(text) => text.to_string(),
                        Err(broadcast::error::RecvError::Lagged(_)) => json!(Push::Resync).to_string(),
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    if out.send(text).await.is_err() {
                        break;
                    }
                }
                _ = tick.tick() => {
                    if out.send(json!(Push::Tick).to_string()).await.is_err() {
                        break;
                    }
                }
            }
        }
    };
    // Covers sends to a full outbox too, so a slow client cannot hold shutdown open.
    let stopping = tokio::select! {
        biased;
        _ = shutdown.cancelled() => true,
        _ = read => false,
        _ = &mut writer => false,
    };
    drop(out);
    // Admitted requests may have side effects, so finish them even after a disconnect.
    let admitted = async { while requests.join_next().await.is_some() {} };
    if stopping {
        // Requests still own senders. Drain their replies alongside the requests until the
        // channel closes; the existing session owner and backend deadline bound this wait.
        futures::future::join(admitted, writer).await;
    } else {
        // A disconnected reader or failed writer cannot receive replies. Dropping the writer
        // releases requests waiting on a full outbox without aborting their side effects.
        drop(writer);
        admitted.await;
    }
}

/// Pushes an instance's changes to every connected GUI: new entries of its event log and the live
/// view of its running turns. Stops with `shutdown`. It reads on the read connection, so polling
/// never waits for a command, nor holds one up.
pub async fn watch(instance: Arc<Instance>, shutdown: CancellationToken) {
    let mut last_seq = db(&instance, |db| db.read(last_event)).await.unwrap_or(0);
    let mut last_live = instance.live_version.load(Ordering::Relaxed);
    let mut last_items: HashMap<String, i64> = db(&instance, |db| db.read(thread_heads)).await.unwrap_or_default();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(WATCH_INTERVAL) => {}
            _ = shutdown.cancelled() => return,
        }
        let after = last_seq;
        match db(&instance, move |db| db.read(|c| events_after(c, after))).await {
            Ok(events) if !events.is_empty() => {
                last_seq = events.last().map_or(last_seq, |e| e.seq);
                instance.push(Push::Events { project: &instance.id, events: &events });
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = format!("{e:#}"), "could not read the event log"),
        }
        // Transcript items are not business events; each thread's newest item tells the GUI to
        // fetch what it has not seen.
        if let Ok(heads) = db(&instance, |db| db.read(thread_heads)).await {
            for (role, seq) in &heads {
                if last_items.get(role) != Some(seq) {
                    instance.push(Push::Thread { project: &instance.id, role, seq: *seq });
                }
            }
            last_items = heads;
        }
        let version = instance.live_version.load(Ordering::Relaxed);
        if version != last_live {
            last_live = version;
            let live = instance.live.lock().unwrap().clone();
            instance.push(Push::Live { project: &instance.id, live: &live });
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

async fn respond(service: &Service, text: &str) -> String {
    let request: Request = match serde_json::from_str(text) {
        Ok(request) => request,
        Err(e) => {
            return json!({ "id": null, "error": RemoteError { code: "bad_request", message: e.to_string() } })
                .to_string();
        }
    };
    match call(service, &request.method, request.params).await {
        Ok(result) => json!({ "id": request.id, "result": result }).to_string(),
        Err(e) => json!({ "id": request.id, "error": RemoteError { code: error_code(&e), message: format!("{e:#}") } })
            .to_string(),
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

/// `command` changes something; every other method reads. `host` and the host's commands need no
/// project.
async fn call(service: &Service, method: &str, params: Value) -> anyhow::Result<Value> {
    match method {
        name if name == view::HOST.name => {
            view::HOST.encode(view::host_snapshot(&service.host, &service.projects, &service.angela).await?)
        }
        "command" => {
            let params: command::CommandParams = serde_json::from_value(params)?;
            if let Some(result) = command::host(&service.host, &service.projects, &params).await? {
                return Ok(result);
            }
            match service.scope(params.project.as_deref())? {
                Scope::Project(project) => {
                    let result = command::run(&project, params).await?;
                    project.wake.notify_one();
                    Ok(result)
                }
                Scope::Angela(angela) => {
                    let result = command::run_angela(&angela, params).await?;
                    angela.wake.notify_one();
                    Ok(result)
                }
            }
        }
        _ => match service.scope(params.get("project").and_then(Value::as_str))? {
            Scope::Project(project) => view::answer(&project, method, params).await,
            Scope::Angela(angela) => view::answer_angela(&angela, &service.projects, method, params).await,
        },
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use crate::Backend;
    use crate::host::{Cli, HarnessConfig};
    use std::task::Poll;
    use tokio::sync::oneshot;

    #[derive(Clone, Copy)]
    enum Ending {
        Flush,
        WriteFailure,
        Disconnect,
        Deadline,
    }

    #[tokio::test]
    async fn shutdown_flushes_admitted_replies_within_its_existing_grace() {
        for ending in [Ending::Flush, Ending::WriteFailure, Ending::Disconnect, Ending::Deadline] {
            let dir = tempfile::tempdir().unwrap();
            let cli = Cli { command: vec!["unused-test-cli".into()], reasoning_effort: None };
            let host = Arc::new(
                Host::open(
                    dir.path(),
                    HarnessConfig {
                        claude: cli.clone(),
                        codex: cli,
                        interrupt_helper: vec![],
                        terminal_helper: "unused".into(),
                        probe_timeout: Duration::from_secs(1),
                    },
                )
                .unwrap(),
            );
            let backend = Backend::start(host.clone(), 0).await.unwrap();
            let service = Service {
                host: host.clone(),
                projects: backend.projects.clone(),
                angela: Err("unused".into()),
                shutdown: backend.serving.clone(),
                sessions: backend.gui_sessions.clone(),
            };
            // The same owner as upgrade(), held through session() and the actual socket flush.
            let owner = service.sessions.token();
            let cancelled = service.shutdown.clone();
            let (incoming, stream) = futures::channel::mpsc::unbounded();
            incoming
                .unbounded_send(Ok(Message::Text(
                    r#"{"id":1,"method":"command","params":{"name":"shutdown","args":{}}}"#.into(),
                )))
                .unwrap();
            let (flushing, started) = oneshot::channel();
            let (release, gate) = oneshot::channel();
            let (flushed, received) = oneshot::channel();
            let sink = futures::sink::unfold(Some((flushing, gate, flushed)), move |mut gates, message| async move {
                if let Message::Text(text) = message {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["id"] == 1 {
                        let (flushing, gate, flushed) = gates.take().unwrap();
                        flushing.send(()).unwrap();
                        gate.await.unwrap();
                        if matches!(ending, Ending::WriteFailure) {
                            return Err(std::io::Error::other("disconnected while flushing"));
                        }
                        flushed.send(value).unwrap();
                    }
                }
                Ok(gates)
            });
            let mut connection = Box::pin(async move {
                session(Box::pin(sink), stream, service).await;
                drop(owner);
            });
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::select! {
                    _ = &mut connection => panic!("session ended before the reply began flushing"),
                    _ = started => {}
                }
                host.shutdown_requested.notified().await;
                if matches!(ending, Ending::Disconnect) {
                    drop(incoming);
                    connection.await;
                    assert!(received.await.is_err(), "a disconnect aborts the blocked writer");
                    backend.shutdown(Duration::from_secs(5)).await;
                    return;
                }
                let expires = matches!(ending, Ending::Deadline);
                let grace = if expires { Duration::ZERO } else { Duration::from_secs(5) };
                let mut stopped = tokio::spawn(backend.shutdown(grace));
                cancelled.cancelled().await;
                // Poll the real session after cancellation, with flush still gated. Aborting
                // its writer here makes this Ready and deterministically loses the reply.
                assert!(matches!(futures::poll!(&mut connection), Poll::Pending), "reply lost during shutdown");
                if expires {
                    (&mut stopped).await.unwrap();
                } else {
                    assert!(!stopped.is_finished(), "the session still owns its pending reply");
                }
                release.send(()).unwrap();
                connection.await;
                match ending {
                    Ending::WriteFailure => assert!(received.await.is_err()),
                    _ => assert_eq!(received.await.unwrap(), json!({"id":1,"result":null})),
                }
                if !expires {
                    stopped.await.unwrap();
                }
            })
            .await
            .expect("session cleanup exceeded the test's watchdog");
        }
    }
}
