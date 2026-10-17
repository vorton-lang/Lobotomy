//! The GUI's reads (frontend.md §7). The models and their queries are `lobotomy_core::view`;
//! here they meet what only the running backend knows: live items, the host's quota domains and
//! permission modes, and whether an unknown turn's process still runs.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use lobotomy_agent::runner::db;
use lobotomy_core::Error;
use lobotomy_core::host::ProjectEntry;
use lobotomy_core::quota::Domain;
use lobotomy_core::view::{self, Attention, Overview, SearchQuery, ThreadQuery};
use lobotomy_harness::{Harness, MCP_SERVER, Model, Permission};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::host::Host;
use crate::instance::{Instance, LiveTurn};
use crate::project::Project;
use crate::projects::Projects;

/// Answers a read method about a project.
pub async fn answer(project: &Arc<Project>, method: &str, params: Value) -> anyhow::Result<Value> {
    Ok(match method {
        name if name == SNAPSHOT.name => SNAPSHOT.encode(snapshot(project).await?)?,
        name if name == TASK.name => {
            let TaskParams { task_id } = TASK.decode(params)?;
            TASK.encode(read(project, move |c| view::task_detail(c, &task_id)).await?)?
        }
        // Asked for when the settings open, not with every snapshot: it walks a directory.
        name if name == DISK_USAGE.name => {
            let turns = project.data_dir.join("turns");
            let (files, bytes) = tokio::task::spawn_blocking(move || directory_size(&turns)).await?;
            DISK_USAGE.encode(DiskUsage { raw_output: RawOutput { files, bytes } })?
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
        name if name == SNAPSHOT.name => SNAPSHOT.encode(angela_snapshot(angela).await?),
        name if name == REFS.name => REFS.encode(refs(angela, projects, REFS.decode(params)?).await?),
        name if name == FORWARDS.name => {
            let ForwardsParams { messages } = FORWARDS.decode(params)?;
            FORWARDS.encode(forwards(angela, projects, messages).await?)
        }
        // The read-only "共识" view: her ledger without her hypotheses (manager-actions.md §3.3).
        name if name == LEDGER.name => LEDGER.encode(crate::mcp::angela::ledgers(angela, projects, None, false).await?),
        _ => transcript(angela, method, params).await,
    }
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct ForwardsParams {
    messages: Vec<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(rename = "Forward"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct RefsParams {
    #[serde(default)]
    quotes: Vec<String>,
    #[serde(default)]
    attachments: Vec<String>,
}

/// What a reference in Angela's reply shows, or why it shows nothing.
#[derive(Serialize)]
#[serde(untagged)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
enum Resolved<T> {
    Found(T),
    Error { error: String },
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct QuotedMessage {
    at: i64,
    body: String,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(rename = "AttachmentView"))]
struct ProjectAttachment {
    project: String,
    project_name: String,
    #[serde(flatten)]
    attachment: view::attachment::Attachment,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
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
        name if name == THREAD.name => {
            let q = THREAD.decode(params)?;
            THREAD.encode(read(instance, move |c| view::thread_page(c, &q)).await?)?
        }
        name if name == SEARCH.name => {
            let q = SEARCH.decode(params)?;
            SEARCH.encode(read(instance, move |c| view::search(c, &q)).await?)?
        }
        name if name == BLOB.name => {
            let BlobParams { hash } = BLOB.decode(params)?;
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::rejected("bad_request", format!("not a blob hash: {hash}")).into());
            }
            BLOB.encode(BlobText { text: instance.blobs.read(&hash)? })?
        }
        other => return Err(Error::rejected("bad_request", format!("unknown method {other}")).into()),
    })
}

/// Reads a model on the read connection.
async fn read<T: Send + 'static>(
    instance: &Instance,
    f: impl FnOnce(&Connection) -> lobotomy_core::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    db(instance, move |db| db.read(f)).await
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct TaskParams {
    task_id: String,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct BlobParams {
    hash: String,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct HarnessView {
    harness: &'static str,
    /// `Permission::as_str`.
    #[cfg_attr(feature = "ts-rs", ts(type = "\"full\" | \"auto_review\""))]
    permission: &'static str,
    /// The models a role can choose on it (harness-adapter.md §1.2); empty when they are not
    /// known, and the GUI then takes a name as typed.
    models: Vec<ModelView>,
    /// The efforts a role on the CLI's default model can choose: those every listed model takes.
    efforts: Vec<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct ModelView {
    id: String,
    label: String,
    efforts: Vec<String>,
    /// The effort it uses when none is chosen, if the CLI says.
    default_effort: Option<String>,
}

impl HarnessView {
    fn new(harness: Harness, permission: Permission, models: Vec<Model>) -> Self {
        let efforts = models.first().map_or_else(Vec::new, |first| {
            let every = |effort: &String| models.iter().all(|m| m.efforts.contains(effort));
            first.efforts.iter().filter(|e| every(e)).cloned().collect()
        });
        let models = models
            .into_iter()
            .map(|m| ModelView { id: m.id, label: m.label, efforts: m.efforts, default_effort: m.default_effort })
            .collect();
        Self { harness: harness.as_str(), permission: permission.as_str(), models, efforts }
    }
}

pub async fn snapshot(project: &Arc<Project>) -> anyhow::Result<Snapshot> {
    let overview = db(project, |db| db.read(|c| view::overview(c, MCP_SERVER))).await?;
    let quota = quota(&project.host, &overview)?;
    let attention = attention(&overview, &quota);
    instance_snapshot(project, overview, attention, quota)
}

async fn angela_snapshot(angela: &Instance) -> anyhow::Result<Snapshot> {
    let overview = db(angela, |db| db.read(|c| view::overview(c, MCP_SERVER))).await?;
    let quota = quota(&angela.host, &overview)?;
    let attention = attention(&overview, &quota);
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

fn attention(overview: &Overview, quota: &[Domain]) -> Vec<Attention> {
    view::attention(overview, quota, |turn| {
        matches!((turn.pid, turn.process_start), (Some(pid), Some(start))
            if lobotomy_harness::process::is_running(pid as u32, start))
    })
}

/// What waits for the user in a project, without the quota domains the host lists once
/// (data-model.md §10.5).
pub fn project_attention(overview: &Overview) -> Vec<Attention> {
    attention(overview, &[])
}

fn harnesses(host: &Arc<Host>) -> anyhow::Result<Vec<HarnessView>> {
    Harness::ALL
        .iter()
        .map(|&harness| Ok(HarnessView::new(harness, host.permission(harness)?, host.models(harness))))
        .collect()
}

/// What the host shows across projects (frontend.md §2, §3 rule 8).
#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
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
) -> anyhow::Result<HostSnapshot> {
    let mut statuses = Vec::new();
    for entry in host.db.projects()? {
        let (mut attention, mut busy, mut error) = (Vec::new(), false, projects.failure(&entry.id));
        if let Some(project) = projects.get(&entry.id) {
            // One project's broken database does not hide the others.
            match db(&project, |db| db.read(|c| view::overview(c, MCP_SERVER))).await {
                Ok(overview) => attention = project_attention(&overview),
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
                attention: attention(&overview, &[]),
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
    Ok(snapshot)
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

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct RawOutput {
    files: u64,
    bytes: u64,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct DiskUsage {
    raw_output: RawOutput,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
struct BlobText {
    text: String,
}

super::protocol::routes! {
    bindings, method;
    pub(super) HOST: super::protocol::NoParams => HostSnapshot = "host";
    SNAPSHOT: super::protocol::NoParams => Snapshot = "snapshot";
    TASK: TaskParams => view::TaskDetail = "task";
    DISK_USAGE: super::protocol::NoParams => DiskUsage = "disk_usage";
    THREAD: ThreadQuery => view::ThreadPage = "thread";
    SEARCH: SearchQuery => view::Found = "search";
    BLOB: BlobParams => BlobText = "blob";
    REFS: RefsParams => Refs = "refs";
    FORWARDS: ForwardsParams => HashMap<String, Vec<ProjectForward>> = "forwards";
    LEDGER: super::protocol::NoParams => crate::mcp::angela::Ledgers = "ledger";
}

/// Representative private/flattened read models for the cross-language contract check.
#[cfg(feature = "ts-rs")]
pub(super) fn binding_fixtures() -> Value {
    let overview = Overview { seq: 1, project: None, roles: vec![], tasks: vec![], questions: vec![] };
    let snapshot = Snapshot {
        overview,
        attention: vec![],
        quota: vec![],
        harnesses: vec![HarnessView::new(
            Harness::Codex,
            Permission::AutoReview,
            vec![Model {
                id: "gpt".into(),
                label: "GPT".into(),
                efforts: vec!["low".into()],
                default_effort: Some("low".into()),
            }],
        )],
        live: HashMap::new(),
    };
    let refs = Refs {
        quotes: HashMap::from([
            ("found".into(), Resolved::Found(QuotedMessage { at: 1, body: "quoted".into() })),
            ("missing".into(), Resolved::Error { error: "missing".into() }),
        ]),
        attachments: HashMap::from([(
            "task".into(),
            Resolved::Found(ProjectAttachment {
                project: "project".into(),
                project_name: "Project".into(),
                attachment: view::attachment::Attachment::Task {
                    task_id: "task".into(),
                    title: "Task".into(),
                    executor: "Malkuth".into(),
                    phase: lobotomy_core::task::Phase::Queued,
                    paused: false,
                },
            }),
        )]),
    };
    serde_json::json!({ "snapshot": snapshot, "refs": refs })
}
