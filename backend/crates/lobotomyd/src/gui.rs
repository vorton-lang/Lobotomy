//! The GUI's connection: one WebSocket per window (frontend.md §1, §3).
//!
//! - The client sends requests `{id, method, params}` and gets `{id, result}` or
//!   `{id, error: {code, message}}`. Commands carry the GUI's request id as their idempotency
//!   key, so a repeated request does nothing twice.
//! - The server pushes what changed, not the content: `{type: "events", events}` with the global
//!   event log, `{type: "live", live}` with items of running turns, `{type: "tick"}` every 15
//!   seconds, and `{type: "resync"}` when the client fell behind and must take a new snapshot.
//! - A connection needs the host's GUI token, and `Host` and `Origin` must be local.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, bail};
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures::{SinkExt as _, StreamExt as _};
use lobotomy_core::capture::{ApproveNewFiles, Capture, DiscardUncaptured};
use lobotomy_core::project::{EditProjectConfig, ProjectConfig, current_config, load_project};
use lobotomy_core::quota::Domain;
use lobotomy_core::role::{Role, list_roles};
use lobotomy_core::task::{
    Abandon, CreateTask, EditCriteria, MoveInQueue, Phase, Reopen, SendMessage, SetPaused, Task, list_tasks, occupant,
    queued_messages,
};
use lobotomy_core::turn::{
    Continue, Hold, NewNativeSession, Outcome, Turn, TurnState, hold, last_turn, load_turn, unfinished_turn,
};
use lobotomy_core::verify::{Accept, RetryPreview, SendBack, Verification, latest_verification, preview_stopped};
use lobotomy_core::workspace::{Workspace, current_workspace};
use lobotomy_core::{Caller, Command, Db};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::project::{LiveTurn, Project};
use crate::results;
use crate::runner::db;

/// How often the event log and the live view are looked at for changes.
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
const TICK: Duration = Duration::from_secs(15);
/// Pushes kept for slow clients. A client further behind gets `resync`.
pub const PUSH_BUFFER: usize = 512;
/// Thread items per page.
const PAGE: i64 = 100;

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
    origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .is_some_and(is_local)
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
/// running turns. Stops with `shutdown`.
pub async fn watch(project: Arc<Project>, shutdown: CancellationToken) {
    let mut last_seq = db(&project, |db| db.read(max_seq)).await.unwrap_or(0);
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
                    let _ = project.gui_push.send(json!({ "type": "thread", "role": role, "seq": seq }).to_string().into());
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

#[derive(Serialize)]
struct EventRow {
    seq: i64,
    kind: String,
    entity: String,
    payload: Value,
}

/// The newest item of each role's thread.
fn thread_heads(conn: &Connection) -> lobotomy_core::Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT t.role, MAX(i.seq) FROM item i JOIN thread t ON t.id = i.thread_id GROUP BY t.role")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn max_seq(conn: &Connection) -> lobotomy_core::Result<i64> {
    Ok(conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM event", [], |r| r.get(0))?)
}

fn events_after(conn: &Connection, seq: i64) -> lobotomy_core::Result<Vec<EventRow>> {
    let mut stmt =
        conn.prepare("SELECT seq, kind, entity, payload FROM event WHERE seq > ?1 ORDER BY seq LIMIT 500")?;
    let rows = stmt.query_map([seq], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, String>(3)?)))?;
    rows.map(|row| {
        let (seq, kind, entity, payload) = row?;
        Ok(EventRow { seq, kind, entity, payload: serde_json::from_str(&payload)? })
    })
    .collect()
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
        Err(e) => return json!({ "id": null, "error": { "code": "bad_request", "message": e.to_string() } }).to_string(),
    };
    match call(project, &request.method, request.params).await {
        Ok(result) => json!({ "id": request.id, "result": result }).to_string(),
        Err(e) => {
            let code = e.downcast_ref::<lobotomy_core::Error>().and_then(|e| e.code()).unwrap_or("internal");
            json!({ "id": request.id, "error": { "code": code, "message": format!("{e:#}") } }).to_string()
        }
    }
}

async fn call(project: &Arc<Project>, method: &str, params: Value) -> anyhow::Result<Value> {
    Ok(match method {
        "snapshot" => serde_json::to_value(snapshot(project).await?)?,
        "thread" => thread(project, serde_json::from_value(params)?).await?,
        "search" => {
            let p: SearchParams = serde_json::from_value(params)?;
            db(project, move |db| db.read(|c| search_thread(c, &p))).await?
        }
        "task" => task_detail(project, serde_json::from_value(params)?).await?,
        "diff" => {
            let DiffParams { from, to } = serde_json::from_value(params)?;
            let store = project.store.clone();
            serde_json::to_value(tokio::task::spawn_blocking(move || store.diff(&from, &to)).await??)?
        }
        "blob" => {
            let BlobParams { hash } = serde_json::from_value(params)?;
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("not a blob hash: {hash}");
            }
            json!({ "text": project.blobs.read(&hash)? })
        }
        "command" => {
            let CommandParams { name, args } = serde_json::from_value(params)?;
            let result = command(project, &name, args).await?;
            project.wake.notify_one();
            result
        }
        other => bail!("unknown method {other}"),
    })
}

// ---- snapshot ----

#[derive(Serialize)]
pub struct Snapshot {
    /// The last event the snapshot includes; apply pushed events after it.
    seq: i64,
    project: Option<lobotomy_core::project::Project>,
    config: Option<ConfigView>,
    roles: Vec<RoleView>,
    tasks: Vec<TaskView>,
    /// What waits for the user ("等你决定", frontend.md §2).
    attention: Vec<Attention>,
    quota: Vec<Domain>,
    live: HashMap<String, LiveTurn>,
}

#[derive(Serialize)]
struct ConfigView {
    version: i64,
    config: ProjectConfig,
}

#[derive(Serialize)]
struct RoleView {
    #[serde(flatten)]
    role: Role,
    task_id: Option<String>,
    unfinished: Option<Turn>,
    last_turn: Option<Turn>,
    hold: Option<Hold>,
    stalled: Option<Stalled>,
    workspace: Option<Workspace>,
    queued_messages: usize,
}

#[derive(Clone, Serialize)]
struct Stalled {
    task_id: String,
    turn_id: String,
    /// Why a call to a Lobotomy tool in that turn recorded nothing, when one did not.
    report_error: Option<String>,
}

/// The role's task is executing, yet nothing will move it on without the user: the last turn of
/// the attempt completed without done or a question, and nothing is queued, running, held or
/// being captured (#13).
fn stalled(conn: &Connection, role: &RoleView) -> lobotomy_core::Result<Option<Stalled>> {
    let (Some(task_id), Some(last)) = (&role.task_id, &role.last_turn) else { return Ok(None) };
    if role.unfinished.is_some()
        || role.hold.is_some()
        || role.queued_messages > 0
        || lobotomy_core::capture::require_no_pending_capture(conn, &role.role.name).is_err()
    {
        return Ok(None);
    }
    let task = lobotomy_core::task::load_task(conn, task_id)?;
    let attempt = lobotomy_core::task::open_attempt(conn, task_id)?;
    if task.phase != Phase::Executing
        || task.paused
        || task.blocked_reason.is_some()
        || last.outcome != Some(Outcome::Completed)
        || last.attempt_id != attempt.map(|a| a.id)
    {
        return Ok(None);
    }
    // A Lobotomy tool call that went through has a command record; one without failed before
    // reaching the runtime, or the runtime refused it.
    let mut stmt = conn.prepare(
        "SELECT content FROM item WHERE turn_id = ?1 AND kind = 'mcp_call' AND command_id IS NULL ORDER BY seq",
    )?;
    let calls = stmt.query_map([&last.id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let report_error = calls
        .iter()
        .filter_map(|c| serde_json::from_str::<Value>(c).ok())
        .filter(|c| c["server"] == "lobotomy")
        .map(|c| {
            let text = c["error"]["message"].as_str().or(c["error"].as_str()).or(c["result"]["content"][0]["text"].as_str());
            match text {
                Some(text) => text.to_owned(),
                None if !c["error"].is_null() => c["error"].to_string(),
                None => "调用没有返回内容".to_owned(),
            }
        })
        .next_back();
    Ok(Some(Stalled { task_id: task_id.clone(), turn_id: last.id.clone(), report_error }))
}

#[derive(Serialize)]
struct TaskView {
    #[serde(flatten)]
    task: Task,
    attempt_seq: Option<i64>,
    attempt_open: bool,
    verification: Option<Verification>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Attention {
    /// The role waits: continue, start a new session, or decide on a stopped capture.
    Hold { role: String, hold: Hold },
    /// The backend restarted while this turn's CLI ran, and the CLI still runs.
    UnknownTurn { role: String, turn_id: String },
    TaskBlocked { task_id: String, title: String, reason: String },
    /// The executor's turn ended normally but it reported neither done nor a question, and
    /// nothing is queued for it. In M1 only the user can move it on (#13).
    Stalled { role: String, task_id: String, title: String, turn_id: String, report_error: Option<String> },
    Accept { task_id: String, title: String, verification_id: String },
    Quota { domain: Domain },
    JobFailed { key: String, reason: String },
    PreviewStopped { reason: String },
}

struct DbView {
    seq: i64,
    project: Option<lobotomy_core::project::Project>,
    config: Option<ConfigView>,
    roles: Vec<RoleView>,
    tasks: Vec<TaskView>,
    preview_stopped: Option<String>,
}

fn db_view(conn: &Connection) -> lobotomy_core::Result<DbView> {
    let seq = max_seq(conn)?;
    let project = load_project(conn)?;
    let config = match project {
        Some(_) => Some(current_config(conn).map(|(version, config)| ConfigView { version, config })?),
        None => None,
    };
    let mut roles = Vec::new();
    for role in list_roles(conn)? {
        let workspace = match &role.slot {
            Some(slot) => current_workspace(conn, slot)?,
            None => None,
        };
        let mut view = RoleView {
            task_id: occupant(conn, &role.name)?,
            unfinished: unfinished_turn(conn, &role.name)?,
            last_turn: last_turn(conn, &role.name)?,
            hold: hold(conn, &role.name)?,
            stalled: None,
            workspace,
            queued_messages: queued_messages(conn, &role.name)?.len(),
            role,
        };
        view.stalled = stalled(conn, &view)?;
        roles.push(view);
    }
    let mut tasks = Vec::new();
    for task in list_tasks(conn)? {
        let attempt: Option<(i64, bool)> = conn
            .query_row(
                "SELECT seq, ended_at IS NULL FROM attempt WHERE task_id = ?1 ORDER BY seq DESC LIMIT 1",
                [&task.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        tasks.push(TaskView {
            attempt_seq: attempt.map(|a| a.0),
            attempt_open: attempt.is_some_and(|a| a.1),
            verification: latest_verification(conn, &task.id)?,
            task,
        });
    }
    let preview_stopped = if project.is_some() { preview_stopped(conn)? } else { None };
    Ok(DbView { seq, project, config, roles, tasks, preview_stopped })
}

pub async fn snapshot(project: &Arc<Project>) -> anyhow::Result<Snapshot> {
    let view = db(project, |db| db.read(db_view)).await?;
    let mut attention = Vec::new();
    for role in &view.roles {
        if let Some(turn) = &role.unfinished
            && turn.state == TurnState::Unknown
            && let (Some(pid), Some(start)) = (turn.pid, turn.process_start)
            && lobotomy_harness::process::is_running(pid as u32, start)
        {
            attention.push(Attention::UnknownTurn { role: role.role.name.clone(), turn_id: turn.id.clone() });
        }
        if let Some(hold) = &role.hold {
            attention.push(Attention::Hold { role: role.role.name.clone(), hold: hold.clone() });
        }
        if let Some(stalled) = &role.stalled {
            let task = view.tasks.iter().find(|t| t.task.id == stalled.task_id).map(|t| &t.task);
            attention.push(Attention::Stalled {
                role: role.role.name.clone(),
                task_id: stalled.task_id.clone(),
                title: task.map(|t| t.title.clone()).unwrap_or_default(),
                turn_id: stalled.turn_id.clone(),
                report_error: stalled.report_error.clone(),
            });
        }
    }
    for task in &view.tasks {
        let t = &task.task;
        if let (Phase::Executing, Some(reason)) = (t.phase, &t.blocked_reason) {
            attention.push(Attention::TaskBlocked { task_id: t.id.clone(), title: t.title.clone(), reason: reason.clone() });
        }
        if t.phase == Phase::Accepting
            && let Some(v) = &task.verification
        {
            attention.push(Attention::Accept { task_id: t.id.clone(), title: t.title.clone(), verification_id: v.id.clone() });
        }
    }
    let harnesses: BTreeSet<&str> = view.roles.iter().map(|r| r.role.harness.as_str()).collect();
    let mut quota = Vec::new();
    for harness in harnesses {
        let domain = project.host.db.domain(harness)?;
        if domain.is_blocked() {
            attention.push(Attention::Quota { domain: domain.clone() });
        }
        quota.push(domain);
    }
    for (key, reason) in results::failures(project) {
        attention.push(Attention::JobFailed { key, reason });
    }
    if let Some(reason) = view.preview_stopped {
        attention.push(Attention::PreviewStopped { reason });
    }
    let live = project.live.lock().unwrap().clone();
    Ok(Snapshot {
        seq: view.seq,
        project: view.project,
        config: view.config,
        roles: view.roles,
        tasks: view.tasks,
        attention,
        quota,
        live,
    })
}

// ---- thread ----

#[derive(Deserialize)]
struct ThreadParams {
    role: String,
    /// Items before this thread sequence number, for scrolling back.
    before: Option<i64>,
    /// Items after this sequence number, oldest first, to catch up. Without `before` or `after`,
    /// the newest page.
    after: Option<i64>,
    limit: Option<i64>,
}

#[derive(Serialize)]
struct ItemRow {
    id: String,
    seq: i64,
    turn_id: String,
    kind: String,
    content: Value,
    command_id: Option<String>,
    created_at: i64,
}

#[derive(Serialize)]
struct MessageRow {
    id: String,
    source: String,
    task_id: Option<String>,
    body: String,
    turn_id: Option<String>,
    created_at: i64,
}

#[derive(Serialize)]
struct CommandRow {
    name: String,
    args: Value,
    result: Value,
}

fn thread_page(conn: &Connection, p: &ThreadParams) -> lobotomy_core::Result<Value> {
    let limit = p.limit.unwrap_or(PAGE).clamp(1, 500);
    let order = if p.after.is_some() { "ASC" } else { "DESC" };
    let mut stmt = conn.prepare(&format!(
        "SELECT i.id, i.seq, i.turn_id, i.kind, i.content, i.command_id, i.created_at
         FROM item i JOIN thread t ON t.id = i.thread_id
         WHERE t.role = ?1 AND (?2 IS NULL OR i.seq < ?2) AND (?3 IS NULL OR i.seq > ?3)
         ORDER BY i.seq {order} LIMIT ?4"
    ))?;
    let rows = stmt.query_map(params![p.role, p.before, p.after, limit], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, String>(4)?, r.get(5)?, r.get(6)?))
    })?;
    let mut items = Vec::new();
    for row in rows {
        let (id, seq, turn_id, kind, content, command_id, created_at) = row?;
        items.push(ItemRow { id, seq, turn_id, kind, content: serde_json::from_str(&content)?, command_id, created_at });
    }
    if p.after.is_none() {
        items.reverse();
    }
    let has_more = match items.first() {
        Some(first) => conn
            .query_row(
                "SELECT 1 FROM item i JOIN thread t ON t.id = i.thread_id WHERE t.role = ?1 AND i.seq < ?2 LIMIT 1",
                params![p.role, first.seq],
                |_| Ok(()),
            )
            .optional()?
            .is_some(),
        None => false,
    };

    let turn_ids: BTreeSet<&str> = items.iter().map(|i| i.turn_id.as_str()).collect();
    let mut turns = HashMap::new();
    let mut messages: Vec<MessageRow> = Vec::new();
    for turn_id in &turn_ids {
        turns.insert(turn_id.to_string(), load_turn(conn, turn_id)?);
        messages.extend(messages_where(conn, "turn_id = ?1", [turn_id])?);
    }
    let mut commands = HashMap::new();
    for id in items.iter().filter_map(|i| i.command_id.as_deref()) {
        let row: Option<(String, String, String)> = conn
            .query_row("SELECT name, args, result FROM command_record WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        if let Some((name, args, result)) = row {
            let row = CommandRow { name, args: serde_json::from_str(&args)?, result: serde_json::from_str(&result)? };
            commands.insert(id.to_owned(), row);
        }
    }
    // Messages still waiting for a turn belong at the end of the newest page.
    let queued = messages_where(conn, "role = ?1 AND state = 'queued'", [&p.role])?;
    Ok(json!({ "items": items, "has_more": has_more, "turns": turns, "messages": messages, "commands": commands, "queued": queued }))
}

fn messages_where(conn: &Connection, filter: &str, args: impl rusqlite::Params) -> lobotomy_core::Result<Vec<MessageRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, source, task_id, body, turn_id, created_at FROM message WHERE {filter} ORDER BY seq"
    ))?;
    let rows = stmt.query_map(args, |r| {
        Ok(MessageRow {
            id: r.get(0)?,
            source: r.get(1)?,
            task_id: r.get(2)?,
            body: r.get(3)?,
            turn_id: r.get(4)?,
            created_at: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

async fn thread(project: &Arc<Project>, p: ThreadParams) -> anyhow::Result<Value> {
    db(project, move |db| db.read(|c| thread_page(c, &p))).await
}

// ---- search ----

#[derive(Deserialize)]
struct SearchParams {
    role: String,
    query: String,
}

/// Where a match is: an item, or a message shown at the start of its turn (`seq` is the turn's
/// first item) or among the queued ones at the end (`seq` is absent).
#[derive(Serialize)]
struct Match {
    kind: &'static str,
    id: String,
    seq: Option<i64>,
}

/// Matches returned at most, the newest; the GUI says when older ones were left out.
const MATCH_LIMIT: usize = 1000;

/// A text field of an item as the thread shows it: a field moved to the blob store is searched
/// by its head and tail, which is what the thread shows of it.
fn shown(field: &str) -> String {
    format!(
        "CASE json_type(i.content, '{field}') WHEN 'text' THEN json_extract(i.content, '{field}') \
         WHEN 'object' THEN coalesce(json_extract(i.content, '{field}.head'), '') || char(10) || \
         coalesce(json_extract(i.content, '{field}.tail'), '') END"
    )
}

/// The thread's matches of `query` in order, ignoring ASCII case (frontend.md §4.1 "搜索"): the
/// text of items, the report a Lobotomy tool call recorded, and messages. The GUI loads pages
/// back to a match before showing it.
fn search_thread(conn: &Connection, p: &SearchParams) -> lobotomy_core::Result<Value> {
    let query = p.query.trim();
    if query.is_empty() {
        return Ok(json!({ "matches": [], "more": false }));
    }
    let text = ["$.text", "$.command", "$.output", "$.query", "$.message"]
        .iter()
        .map(|f| format!("coalesce({}, '')", shown(f)))
        .chain(["title", "body", "blocked_on"].iter().map(|k| format!("coalesce(json_extract(c.args, '$.{k}'), '')")))
        .collect::<Vec<_>>()
        .join(" || char(10) || ");
    let mut stmt = conn.prepare(&format!(
        "SELECT i.id, i.seq FROM item i JOIN thread t ON t.id = i.thread_id
         LEFT JOIN command_record c ON c.id = i.command_id
         WHERE t.role = ?1 AND i.kind != 'input' AND instr(lower({text}), lower(?2)) > 0
         ORDER BY i.seq DESC LIMIT ?3"
    ))?;
    let limit = MATCH_LIMIT as i64 + 1;
    let items = stmt.query_map(params![p.role, query, limit], |r| Ok(Match { kind: "item", id: r.get(0)?, seq: Some(r.get(1)?) }))?;
    let mut matches = items.collect::<rusqlite::Result<Vec<_>>>()?;
    // A message bound to a turn that never stored an item is not shown, so it does not match.
    let mut stmt = conn.prepare(
        "SELECT m.id, m.turn_id, (SELECT MIN(i.seq) FROM item i WHERE i.turn_id = m.turn_id) FROM message m
         WHERE m.role = ?1 AND instr(lower(m.body), lower(?2)) > 0 ORDER BY m.seq",
    )?;
    let messages = stmt.query_map(params![p.role, query], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<i64>>(2)?))
    })?;
    for row in messages {
        let (id, turn, seq) = row?;
        if turn.is_none() || seq.is_some() {
            matches.push(Match { kind: "message", id, seq });
        }
    }
    // The thread's order: a turn's messages come before its items; queued messages come last.
    matches.sort_by_key(|m| (m.seq.unwrap_or(i64::MAX), m.kind == "item"));
    let more = matches.len() > MATCH_LIMIT;
    if more {
        matches.drain(..matches.len() - MATCH_LIMIT);
    }
    Ok(json!({ "matches": matches, "more": more }))
}

// ---- task detail ----

#[derive(Deserialize)]
struct TaskParams {
    task_id: String,
}

fn rows_as_json(conn: &Connection, sql: &str, task_id: &str, json_columns: &[&str]) -> lobotomy_core::Result<Vec<Value>> {
    let mut stmt = conn.prepare(sql)?;
    let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
    let mut rows = stmt.query([task_id])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut object = serde_json::Map::new();
        for (i, name) in names.iter().enumerate() {
            let value = match row.get_ref(i)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(n) => json!(n),
                rusqlite::types::ValueRef::Real(f) => json!(f),
                rusqlite::types::ValueRef::Text(t) => {
                    let text = String::from_utf8_lossy(t);
                    if json_columns.contains(&name.as_str()) {
                        serde_json::from_str(&text).unwrap_or(Value::String(text.into_owned()))
                    } else {
                        Value::String(text.into_owned())
                    }
                }
                rusqlite::types::ValueRef::Blob(_) => Value::Null,
            };
            object.insert(name.clone(), value);
        }
        out.push(Value::Object(object));
    }
    Ok(out)
}

fn task_view(conn: &Connection, task_id: &str) -> lobotomy_core::Result<Value> {
    let task = lobotomy_core::task::load_task(conn, task_id)?;
    let criteria = rows_as_json(
        conn,
        "SELECT version, text, created_by, created_at FROM criteria_version WHERE task_id = ?1 ORDER BY version",
        task_id,
        &[],
    )?;
    let attempts = rows_as_json(
        conn,
        "SELECT id, seq, started_at, ended_at, end_reason, code_start, candidate_id, done_turn_id
         FROM attempt WHERE task_id = ?1 ORDER BY seq",
        task_id,
        &[],
    )?;
    let captures: Vec<Capture> = {
        let ids: Vec<String> = {
            let mut stmt = conn.prepare("SELECT id FROM capture WHERE task_id = ?1 ORDER BY created_at, id")?;
            stmt.query_map([task_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        ids.iter().map(|id| lobotomy_core::capture::load_capture(conn, id)).collect::<lobotomy_core::Result<_>>()?
    };
    let verifications = rows_as_json(
        conn,
        "SELECT id, attempt_id, capture_id, base, config_version, commit_id, conflicts, state, created_at, finished_at
         FROM verification WHERE task_id = ?1 ORDER BY created_at, id",
        task_id,
        &["conflicts"],
    )?;
    let checks = rows_as_json(
        conn,
        "SELECT c.id, c.verification_id, c.seq, c.command, c.exit_code, c.timed_out, c.output, c.duration_ms
         FROM check_run c JOIN verification v ON v.id = c.verification_id WHERE v.task_id = ?1
         ORDER BY v.created_at, c.seq",
        task_id,
        &["output"],
    )?;
    let decisions = rows_as_json(
        conn,
        "SELECT id, kind, actor, detail, created_at FROM decision WHERE task_id = ?1 ORDER BY created_at, id",
        task_id,
        &["detail"],
    )?;
    let publications = rows_as_json(
        conn,
        "SELECT rev, commit_id, previous, created_at FROM publication WHERE task_id = ?1 ORDER BY rev",
        task_id,
        &[],
    )?;
    Ok(json!({
        "task": task,
        "criteria": criteria,
        "attempts": attempts,
        "captures": captures,
        "verifications": verifications,
        "checks": checks,
        "decisions": decisions,
        "publications": publications,
    }))
}

async fn task_detail(project: &Arc<Project>, p: TaskParams) -> anyhow::Result<Value> {
    db(project, move |db| db.read(|c| task_view(c, &p.task_id))).await
}

#[derive(Deserialize)]
struct DiffParams {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct BlobParams {
    hash: String,
}

// ---- commands ----

#[derive(Deserialize)]
struct CommandParams {
    name: String,
    #[serde(default)]
    args: Value,
}

async fn user<C>(project: &Arc<Project>, args: Value) -> anyhow::Result<Value>
where
    C: Command + serde::de::DeserializeOwned + Send + 'static,
    C::Output: Send + 'static,
{
    let cmd: C = serde_json::from_value(args).with_context(|| format!("arguments of {}", C::NAME))?;
    let out = db(project, move |db: &Db| db.execute(&Caller::User, &cmd)).await?;
    Ok(serde_json::to_value(out)?)
}

#[derive(Deserialize)]
struct TurnParams {
    turn_id: String,
}

#[derive(Deserialize)]
struct OnboardParams {
    repo_path: String,
}

#[derive(Deserialize)]
struct HarnessParams {
    harness: String,
}

/// The user's commands (data-model.md §9.2) and the runtime actions the GUI may trigger.
async fn command(project: &Arc<Project>, name: &str, args: Value) -> anyhow::Result<Value> {
    macro_rules! user_commands {
        ($($ty:ty),* $(,)?) => {
            $(if name == <$ty as Command>::NAME { return user::<$ty>(project, args).await; })*
        };
    }
    user_commands!(
        CreateTask,
        EditCriteria,
        SendMessage,
        SetPaused,
        MoveInQueue,
        Abandon,
        Reopen,
        Accept,
        SendBack,
        Continue,
        NewNativeSession,
        ApproveNewFiles,
        DiscardUncaptured,
        RetryPreview,
        EditProjectConfig,
    );
    match name {
        "onboard" => {
            let OnboardParams { repo_path } = serde_json::from_value(args)?;
            crate::onboard::onboard(project, std::path::Path::new(&repo_path)).await?;
            Ok(Value::Null)
        }
        "interrupt" => {
            let TurnParams { turn_id } = serde_json::from_value(args)?;
            crate::runner::interrupt(project, &turn_id).await?;
            Ok(Value::Null)
        }
        // "终止残留进程" (data-model.md §3.3): only the CLI of an unknown turn that still runs.
        "terminate" => {
            let TurnParams { turn_id } = serde_json::from_value(args)?;
            let id = turn_id.clone();
            let turn = db(project, move |db| db.read(|c| load_turn(c, &id))).await?;
            let (TurnState::Unknown, Some(pid), Some(start)) = (turn.state, turn.pid, turn.process_start) else {
                bail!("turn {turn_id} is not an unknown turn with a known process");
            };
            Ok(json!({ "terminated": lobotomy_harness::process::terminate(pid as u32, start)? }))
        }
        "quota_retry" => {
            let HarnessParams { harness } = serde_json::from_value(args)?;
            Ok(serde_json::to_value(project.host.retry(&harness).await?)?)
        }
        "retry_failed" => Ok(json!({ "retried": results::retry_failed(project) })),
        "shutdown" => {
            project.shutdown_requested.notify_one();
            Ok(Value::Null)
        }
        other => bail!("unknown command {other}"),
    }
}
