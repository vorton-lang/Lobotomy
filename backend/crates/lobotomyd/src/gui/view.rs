//! The GUI's reads (frontend.md §7). The models and their queries are `lobotomy_core::view`;
//! here they meet what only the running backend knows: live items, the host's quota domains and
//! permission modes, failed store jobs, and whether an unknown turn's process still runs.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use lobotomy_core::Error;
use lobotomy_core::host::ProjectEntry;
use lobotomy_core::quota::Domain;
use lobotomy_core::view::{self, Attention, Overview, SearchQuery, ThreadQuery};
use lobotomy_harness::{Harness, MCP_SERVER};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host::Host;
use crate::instance::{Instance, LiveTurn};
use crate::project::Project;
use crate::projects::Projects;
use crate::results;
use crate::runner::db;

/// Answers a read method about a project.
pub async fn answer(project: &Arc<Project>, method: &str, params: Value) -> anyhow::Result<Value> {
    Ok(match method {
        "trials" => {
            let TaskParams { task_id } = serde_json::from_value(params)?;
            serde_json::to_value(project.trials.list(&task_id))?
        }
        "snapshot" => serde_json::to_value(snapshot(project).await?)?,
        "task" => {
            let TaskParams { task_id } = serde_json::from_value(params)?;
            read(project, move |c| view::task_detail(c, &task_id)).await?
        }
        // Asked for when the settings open, not with every snapshot: it walks a directory.
        "disk_usage" => {
            let turns = project.data_dir.join("turns");
            let (files, bytes) = tokio::task::spawn_blocking(move || directory_size(&turns)).await?;
            json!({ "raw_output": { "files": files, "bytes": bytes } })
        }
        "diff" => {
            let DiffParams { from, to } = serde_json::from_value(params)?;
            let store = project.store.clone();
            serde_json::to_value(tokio::task::spawn_blocking(move || store.diff(&from, &to)).await??)?
        }
        _ => return transcript(project, method, params).await,
    })
}

/// Answers a read method about Angela (data-model.md §10.6). Her snapshot has the shape of a
/// project's, without a repository and tasks, so the window shows her as it shows an executor.
pub async fn answer_angela(
    angela: &Instance,
    projects: &Projects,
    method: &str,
    params: Value,
) -> anyhow::Result<Value> {
    match method {
        "snapshot" => Ok(serde_json::to_value(angela_snapshot(angela).await?)?),
        "refs" => Ok(serde_json::to_value(refs(angela, projects, serde_json::from_value(params)?).await?)?),
        "forwards" => {
            let ForwardsParams { messages } = serde_json::from_value(params)?;
            Ok(serde_json::to_value(forwards(angela, projects, messages).await?)?)
        }
        // The read-only "共识" view: her ledger without her hypotheses (manager-actions.md §3.3).
        "ledger" => Ok(serde_json::to_value(crate::mcp::angela::ledgers(angela, projects, None, false).await?)?),
        _ => transcript(angela, method, params).await,
    }
}

#[derive(Deserialize)]
struct ForwardsParams {
    messages: Vec<String>,
}

#[derive(Serialize)]
struct ProjectForward {
    project: String,
    project_name: String,
    #[serde(flatten)]
    forward: view::manager::Forward,
}

/// Where Angela forwarded each of the user's messages, in every open project ("已转给 …",
/// manager-actions.md §1).
async fn forwards(
    angela: &Instance,
    projects: &Projects,
    messages: Vec<String>,
) -> anyhow::Result<HashMap<String, Vec<ProjectForward>>> {
    let mut out: HashMap<String, Vec<ProjectForward>> = messages.iter().map(|m| (m.clone(), Vec::new())).collect();
    for entry in angela.host.db.projects()? {
        let Some(project) = projects.get(&entry.id) else { continue };
        let ids = messages.clone();
        let found = db(&project, move |db| {
            db.read(|c| {
                ids.iter()
                    .map(|id| Ok((id.clone(), view::manager::forwards(c, id)?)))
                    .collect::<lobotomy_core::Result<Vec<_>>>()
            })
        })
        .await?;
        for (id, list) in found {
            let into = out.entry(id).or_default();
            into.extend(list.into_iter().map(|forward| ProjectForward {
                project: entry.id.clone(),
                project_name: entry.name.clone(),
                forward,
            }));
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct RefsParams {
    #[serde(default)]
    quotes: Vec<String>,
    #[serde(default)]
    attachments: Vec<String>,
}

/// What a reference in Angela's reply shows, or why it shows nothing.
#[derive(Serialize)]
#[serde(untagged)]
enum Resolved<T> {
    Found(T),
    Error { error: String },
}

#[derive(Serialize)]
struct QuotedMessage {
    at: i64,
    body: String,
}

#[derive(Serialize)]
struct ProjectAttachment {
    project: String,
    project_name: String,
    #[serde(flatten)]
    attachment: view::attachment::Attachment,
}

#[derive(Serialize)]
struct Refs {
    quotes: HashMap<String, Resolved<QuotedMessage>>,
    attachments: HashMap<String, Resolved<ProjectAttachment>>,
}

/// The references in Angela's replies, rendered by the runtime (manager-actions.md §2.1):
/// `[[quote:<msg_id>]]` is the user's own message in her database; `[[att:<id>]]` is a record of
/// one of the projects, with whether it still holds. A reference into a project that cannot be
/// read now says so, rather than that nothing has the id (data-model.md §10.6 "读取带边界").
async fn refs(angela: &Instance, projects: &Projects, params: RefsParams) -> anyhow::Result<Refs> {
    let ids = params.quotes;
    let quotes = db(angela, move |db| {
        db.read(|c| {
            let mut out = HashMap::new();
            for id in ids {
                let row: Option<(String, i64, String)> = c
                    .query_row("SELECT source, created_at, body FROM message WHERE id = ?1", [&id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })
                    .optional()?;
                let resolved = match row {
                    Some((source, at, body)) if source == "user" => Resolved::Found(QuotedMessage { at, body }),
                    Some(_) => Resolved::Error { error: "这条消息不是你说的".into() },
                    None => Resolved::Error { error: "没有这条消息".into() },
                };
                out.insert(id, resolved);
            }
            Ok(out)
        })
    })
    .await?;

    let mut open = Vec::new();
    let mut unreadable = Vec::new();
    for entry in angela.host.db.projects()? {
        match projects.get(&entry.id) {
            Some(project) => open.push((entry.name, project)),
            None => unreadable.push(entry.name),
        }
    }
    let mut attachments = HashMap::new();
    for id in params.attachments {
        let mut found = None;
        for (name, project) in &open {
            let key = id.clone();
            if let Some(attachment) = db(project, move |db| db.read(|c| view::attachment::attachment(c, &key))).await? {
                found = Some(ProjectAttachment { project: project.id.clone(), project_name: name.clone(), attachment });
                break;
            }
        }
        let resolved = match found {
            Some(found) => Resolved::Found(found),
            None if unreadable.is_empty() => Resolved::Error { error: "找不到这个引用".into() },
            None => Resolved::Error {
                error: format!("在打开的项目中找不到；读不到的项目：{}", unreadable.join("、"))
            },
        };
        attachments.insert(id, resolved);
    }
    Ok(Refs { quotes, attachments })
}

/// The reads every instance answers: its transcript, searches in it and its blobs.
async fn transcript(instance: &Instance, method: &str, params: Value) -> anyhow::Result<Value> {
    Ok(match method {
        "thread" => {
            let q: ThreadQuery = serde_json::from_value(params)?;
            read(instance, move |c| view::thread_page(c, &q)).await?
        }
        "search" => {
            let q: SearchQuery = serde_json::from_value(params)?;
            read(instance, move |c| view::search(c, &q)).await?
        }
        "blob" => {
            let BlobParams { hash } = serde_json::from_value(params)?;
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::rejected("bad_request", format!("not a blob hash: {hash}")).into());
            }
            json!({ "text": instance.blobs.read(&hash)? })
        }
        other => return Err(Error::rejected("bad_request", format!("unknown method {other}")).into()),
    })
}

/// Reads a model on the read connection.
async fn read<T: Serialize + Send + 'static>(
    instance: &Instance,
    f: impl FnOnce(&Connection) -> lobotomy_core::Result<T> + Send + 'static,
) -> anyhow::Result<Value> {
    Ok(serde_json::to_value(db(instance, move |db| db.read(f)).await?)?)
}

#[derive(Deserialize)]
struct TaskParams {
    task_id: String,
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

#[derive(Serialize)]
pub struct Snapshot {
    #[serde(flatten)]
    overview: Overview,
    /// What waits for the user ("等你决定", frontend.md §2).
    attention: Vec<Attention>,
    quota: Vec<Domain>,
    /// Host settings of each harness Lobotomy runs, shared by all projects.
    harnesses: Vec<HarnessView>,
    live: HashMap<String, LiveTurn>,
}

#[derive(Serialize)]
struct HarnessView {
    harness: &'static str,
    permission: &'static str,
}

pub async fn snapshot(project: &Arc<Project>) -> anyhow::Result<Snapshot> {
    let overview = db(project, |db| db.read(|c| view::overview(c, MCP_SERVER))).await?;
    let quota = quota(&project.host, &overview)?;
    let attention = attention(&overview, &quota, &results::failures(project));
    instance_snapshot(project, overview, attention, quota)
}

async fn angela_snapshot(angela: &Instance) -> anyhow::Result<Snapshot> {
    let overview = db(angela, |db| db.read(|c| view::overview(c, MCP_SERVER))).await?;
    let quota = quota(&angela.host, &overview)?;
    let attention = attention(&overview, &quota, &[]);
    instance_snapshot(angela, overview, attention, quota)
}

fn instance_snapshot(
    instance: &Instance,
    overview: Overview,
    attention: Vec<Attention>,
    quota: Vec<Domain>,
) -> anyhow::Result<Snapshot> {
    let harnesses = harnesses(&instance.host)?;
    let live = instance.live.lock().unwrap().clone();
    Ok(Snapshot { overview, attention, quota, harnesses, live })
}

/// The quota domains of the harnesses the roles use.
fn quota(host: &Host, overview: &Overview) -> anyhow::Result<Vec<Domain>> {
    let used: BTreeSet<&str> = overview.roles.iter().map(|r| r.role.harness.as_str()).collect();
    Ok(used.into_iter().map(|harness| host.db.domain(harness)).collect::<Result<Vec<_>, _>>()?)
}

fn attention(overview: &Overview, quota: &[Domain], failed: &[(String, String)]) -> Vec<Attention> {
    view::attention(overview, quota, failed, |turn| {
        matches!((turn.pid, turn.process_start), (Some(pid), Some(start))
            if lobotomy_harness::process::is_running(pid as u32, start))
    })
}

/// What waits for the user in a project, without the quota domains the host lists once
/// (data-model.md §10.5).
pub fn project_attention(project: &Project, overview: &Overview) -> Vec<Attention> {
    attention(overview, &[], &results::failures(project))
}

fn harnesses(host: &Host) -> anyhow::Result<Vec<HarnessView>> {
    Harness::ALL
        .iter()
        .map(|&harness| Ok(HarnessView { harness: harness.as_str(), permission: host.permission(harness)?.as_str() }))
        .collect()
}

/// What the host shows across projects (frontend.md §2 "M3 的布局", §3 rule 8).
#[derive(Serialize)]
pub struct HostSnapshot {
    projects: Vec<ProjectStatus>,
    /// What waits for the user outside any project: the blocked quota domains, each once
    /// (data-model.md §10.5).
    attention: Vec<Attention>,
    harnesses: Vec<HarnessView>,
    angela: AngelaStatus,
}

/// Angela as the host shows her (data-model.md §10.6). Projects run without her.
#[derive(Serialize)]
struct AngelaStatus {
    /// Why her instance could not be opened; she does not run.
    error: Option<String>,
    /// What of hers waits for the user, such as a turn to continue; her quota domain is in the
    /// host's own list.
    attention: Vec<Attention>,
    /// A turn of hers is running.
    busy: bool,
}

#[derive(Serialize)]
struct ProjectStatus {
    #[serde(flatten)]
    entry: ProjectEntry,
    /// Why a running project could not be opened; it does not run (data-model.md §10.3).
    error: Option<String>,
    /// Its "等你决定", without the quota domains the host lists.
    attention: Vec<Attention>,
    /// A turn of it is running.
    busy: bool,
}

pub async fn host_snapshot(
    host: &Arc<Host>,
    projects: &Projects,
    angela: &Result<Arc<Instance>, String>,
) -> anyhow::Result<Value> {
    let mut statuses = Vec::new();
    for entry in host.db.projects()? {
        let (mut attention, mut busy, mut error) = (Vec::new(), false, projects.failure(&entry.id));
        if let Some(project) = projects.get(&entry.id) {
            // One project's broken database does not hide the others.
            match db(&project, |db| db.read(|c| view::overview(c, MCP_SERVER))).await {
                Ok(overview) => attention = project_attention(&project, &overview),
                Err(e) => error = Some(format!("{e:#}")),
            }
            busy = !project.running.lock().unwrap().is_empty();
        }
        statuses.push(ProjectStatus { error, entry, attention, busy });
    }
    let angela = match angela {
        Ok(angela) => match db(angela, |db| db.read(|c| view::overview(c, MCP_SERVER))).await {
            Ok(overview) => AngelaStatus {
                error: None,
                attention: attention(&overview, &[], &[]),
                busy: !angela.running.lock().unwrap().is_empty(),
            },
            Err(e) => AngelaStatus { error: Some(format!("{e:#}")), attention: Vec::new(), busy: false },
        },
        Err(error) => AngelaStatus { error: Some(error.clone()), attention: Vec::new(), busy: false },
    };
    let attention = Harness::ALL
        .iter()
        .map(|harness| host.db.domain(harness.as_str()))
        .filter(|domain| domain.as_ref().map_or(true, Domain::is_blocked))
        .map(|domain| Ok(Attention::Quota { domain: domain? }))
        .collect::<anyhow::Result<_>>()?;
    let snapshot = HostSnapshot { projects: statuses, attention, harnesses: harnesses(host)?, angela };
    Ok(serde_json::to_value(snapshot)?)
}

/// The files directly in `dir` and their total size; a directory that does not exist is empty.
/// The raw output a turn keeps (data-model.md §7.4) is not deleted on its own yet, so the user
/// sees what it takes (#16).
fn directory_size(dir: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return (0, 0) };
    entries
        .filter_map(|entry| entry.ok()?.metadata().ok())
        .filter(|meta| meta.is_file())
        .fold((0, 0), |(files, bytes), meta| (files + 1, bytes + meta.len()))
}
