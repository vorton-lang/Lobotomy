//! The protocol on the worker host's stdin and stdout (worker-interface.md §3, §9, §10). Requests
//! are handled one at a time, in order. Everything written goes through one writer, so a
//! subscription's snapshot comes before the events after it.

use std::collections::BTreeMap;
use std::sync::Arc;

use lobotomy_worker_protocol::event::{ArtifactKind, EVENT, EvidenceSource, RESOURCES};
use lobotomy_worker_protocol::ops::{
    Ack, Adopt, AgentCapabilities, Capabilities, Close, CreateTask, EnvironmentCapabilities, Hello, HelloResult,
    Revise, SendMessage, Snapshot, Stop, Subscribe,
};
use lobotomy_worker_protocol::rpc::{self, Message, Notification, Request, Response, RpcError};
use lobotomy_worker_protocol::{Method, PROTOCOL_VERSION};
use tokio::io::{AsyncBufRead, AsyncWrite};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::host::Outgoing;
use crate::store;
use crate::task::{Tasks, rpc_error};

/// Serves the orchestrator until it closes `input`.
pub async fn serve(
    tasks: Arc<Tasks>,
    mut input: impl AsyncBufRead + Unpin,
    mut output: impl AsyncWrite + Unpin + Send + 'static,
) -> anyhow::Result<()> {
    let (out, mut outbox) = mpsc::channel::<Message>(256);
    let writer = tokio::spawn(async move {
        while let Some(message) = outbox.recv().await {
            if let Err(e) = rpc::write(&mut output, &message).await {
                tracing::error!(error = %e, "could not write to the orchestrator");
                return;
            }
        }
    });
    let mut forwarding: Option<JoinHandle<()>> = None;
    loop {
        let request = match rpc::read(&mut input).await {
            Ok(Some(Message::Request(request))) => request,
            Ok(Some(other)) => {
                tracing::warn!(?other, "ignored a message that is not a request");
                continue;
            }
            Ok(None) => break,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                tracing::warn!(error = %e, "ignored a line that is not a message");
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if request.method == Subscribe::NAME {
            // The receiver comes first: nothing stored after the snapshot is missed.
            let receiver = tasks.host.outgoing.subscribe();
            match subscribe(&tasks, &request).await {
                Ok((snapshot, replay)) => {
                    let _ = out.send(Message::Response(Response::ok::<Subscribe>(request.id, &snapshot))).await;
                    if let Some(previous) = forwarding.take() {
                        previous.abort();
                    }
                    let known = snapshot.tasks.iter().map(|t| (t.task_id.clone(), t.seq)).collect();
                    forwarding = Some(tokio::spawn(forward(tasks.clone(), out.clone(), receiver, replay, known)));
                }
                Err(e) => {
                    let _ = out.send(Message::Response(Response::err(request.id, e))).await;
                }
            }
            continue;
        }
        let response = handle(&tasks, request).await;
        let _ = out.send(Message::Response(response)).await;
    }
    if let Some(forwarding) = forwarding {
        forwarding.abort();
    }
    drop(out);
    let _ = writer.await;
    Ok(())
}

async fn handle(tasks: &Arc<Tasks>, request: Request) -> Response {
    let id = request.id.clone();
    macro_rules! call {
        ($method:ty, $op:expr) => {
            match request.params::<$method>() {
                Ok(params) => match $op(params).await {
                    Ok(result) => Response::ok::<$method>(id, &result),
                    Err(e) => Response::err(id, e),
                },
                Err(e) => Response::err(id, e),
            }
        };
    }
    match request.method.as_str() {
        Hello::NAME => call!(Hello, |p| hello(tasks, p)),
        CreateTask::NAME => call!(CreateTask, |p| tasks.create_task(p)),
        SendMessage::NAME => call!(SendMessage, |p| tasks.send_message(p)),
        Revise::NAME => call!(Revise, |p| tasks.revise(p)),
        Stop::NAME => call!(Stop, |p| tasks.stop(p)),
        Close::NAME => call!(Close, |p| async { tasks.close(p).await.map(|()| Ack {}) }),
        Adopt::NAME => call!(Adopt, |p| tasks.adopt(p)),
        other => Response::err(id, RpcError::new(rpc::METHOD_NOT_FOUND, format!("no method {other}"))),
    }
}

async fn hello(tasks: &Tasks, params: lobotomy_worker_protocol::ops::HelloParams) -> Result<HelloResult, RpcError> {
    if !params.protocols.iter().any(|p| p == PROTOCOL_VERSION) {
        return Err(RpcError::rejected(
            "unsupported_protocol",
            format!("this worker host speaks {PROTOCOL_VERSION}, not {}", params.protocols.join(", ")),
        ));
    }
    let config = &tasks.host.config;
    let (repo, forge) = (tasks.host.environment.is_some(), tasks.host.forge.is_some());
    Ok(HelloResult {
        protocol: PROTOCOL_VERSION.into(),
        capabilities: Capabilities {
            agent: AgentCapabilities {
                harness: config.harness.as_str().into(),
                models: config.model.iter().cloned().collect(),
                resume: true,
                steer: false,
                interrupt: true,
            },
            environment: EnvironmentCapabilities {
                artifacts: match repo {
                    true => vec![ArtifactKind::Change, ArtifactKind::Report],
                    false => vec![ArtifactKind::Report],
                },
                evidence: match forge {
                    true => vec![EvidenceSource::Environment],
                    false => Vec::new(),
                },
                isolation: if repo { "local-folder" } else { "folder" }.into(),
                platform: std::env::consts::OS.into(),
                adopt: forge,
            },
        },
    })
}

type Replay = Vec<lobotomy_worker_protocol::event::Event>;

async fn subscribe(tasks: &Tasks, request: &Request) -> Result<(Snapshot, Replay), RpcError> {
    let params = request.params::<Subscribe>()?;
    let host = tasks.host.clone();
    tokio::task::spawn_blocking(move || {
        host.store.read(|c| {
            let snapshot = Snapshot { tasks: store::snapshot(c)? };
            let mut replay = Vec::new();
            for (task_id, after) in &params.after {
                replay.extend(store::events_after(c, task_id, *after)?);
            }
            Ok((snapshot, replay))
        })
    })
    .await
    .map_err(|e| rpc_error(e.into()))?
    .map_err(rpc_error)
}

/// Sends the replayed events, the resources, then what comes after the snapshot. Events already
/// in the snapshot are not sent again; after a lag, the missed ones are read back from the store.
async fn forward(
    tasks: Arc<Tasks>,
    out: mpsc::Sender<Message>,
    mut receiver: broadcast::Receiver<Outgoing>,
    replay: Replay,
    mut known: BTreeMap<String, u64>,
) {
    for event in replay {
        if out.send(note(EVENT, &event)).await.is_err() {
            return;
        }
    }
    if out.send(note(RESOURCES, &tasks.host.resources())).await.is_err() {
        return;
    }
    loop {
        let events = match receiver.recv().await {
            Ok(Outgoing::Event(event)) => vec![event],
            Ok(Outgoing::Resources(resources)) => {
                if out.send(note(RESOURCES, &resources)).await.is_err() {
                    return;
                }
                continue;
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let (host, after) = (tasks.host.clone(), known.clone());
                let read = tokio::task::spawn_blocking(move || {
                    host.store.read(|c| {
                        let mut events = Vec::new();
                        for task in store::tasks(c)? {
                            events.extend(store::events_after(c, &task.id, after.get(&task.id).copied().unwrap_or(0))?);
                        }
                        Ok(events)
                    })
                })
                .await;
                match read {
                    Ok(Ok(events)) => events,
                    _ => return,
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        };
        for event in events {
            let seen = known.entry(event.task_id.clone()).or_insert(0);
            if event.seq <= *seen {
                continue;
            }
            *seen = event.seq;
            if out.send(note(EVENT, &event)).await.is_err() {
                return;
            }
        }
    }
}

fn note(method: &str, params: &impl serde::Serialize) -> Message {
    Message::Notification(Notification::new(method, params))
}
