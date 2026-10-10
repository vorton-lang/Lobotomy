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
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host::Host;
use crate::project::{LiveTurn, Project};
use crate::projects::Projects;
use crate::results;
use crate::runner::db;

/// Answers a read method.
pub async fn answer(project: &Arc<Project>, method: &str, params: Value) -> anyhow::Result<Value> {
    Ok(match method {
        "trials" => {
            let TaskParams { task_id } = serde_json::from_value(params)?;
            serde_json::to_value(project.trials.list(&task_id))?
        }
        "snapshot" => serde_json::to_value(snapshot(project).await?)?,
        "thread" => {
            let q: ThreadQuery = serde_json::from_value(params)?;
            read(project, move |c| view::thread_page(c, &q)).await?
        }
        "search" => {
            let q: SearchQuery = serde_json::from_value(params)?;
            read(project, move |c| view::search(c, &q)).await?
        }
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
        "blob" => {
            let BlobParams { hash } = serde_json::from_value(params)?;
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::rejected("bad_request", format!("not a blob hash: {hash}")).into());
            }
            json!({ "text": project.blobs.read(&hash)? })
        }
        other => return Err(Error::rejected("bad_request", format!("unknown method {other}")).into()),
    })
}

/// Reads a model on the read connection.
async fn read<T: Serialize + Send + 'static>(
    project: &Arc<Project>,
    f: impl FnOnce(&Connection) -> lobotomy_core::Result<T> + Send + 'static,
) -> anyhow::Result<Value> {
    Ok(serde_json::to_value(db(project, move |db| db.read(f)).await?)?)
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
    let used: BTreeSet<&str> = overview.roles.iter().map(|r| r.role.harness.as_str()).collect();
    let quota = used.into_iter().map(|harness| project.host.db.domain(harness)).collect::<Result<Vec<_>, _>>()?;
    let attention = attention(project, &overview, &quota);
    let harnesses = harnesses(&project.host)?;
    let live = project.live.lock().unwrap().clone();
    Ok(Snapshot { overview, attention, quota, harnesses, live })
}

fn attention(project: &Project, overview: &Overview, quota: &[Domain]) -> Vec<Attention> {
    view::attention(overview, quota, &results::failures(project), |turn| {
        matches!((turn.pid, turn.process_start), (Some(pid), Some(start))
            if lobotomy_harness::process::is_running(pid as u32, start))
    })
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

pub async fn host_snapshot(host: &Arc<Host>, projects: &Projects) -> anyhow::Result<Value> {
    let mut statuses = Vec::new();
    for entry in host.db.projects()? {
        let (mut attention, mut busy, mut error) = (Vec::new(), false, projects.failure(&entry.id));
        if let Some(project) = projects.get(&entry.id) {
            // One project's broken database does not hide the others.
            match db(&project, |db| db.read(|c| view::overview(c, MCP_SERVER))).await {
                Ok(overview) => attention = self::attention(&project, &overview, &[]),
                Err(e) => error = Some(format!("{e:#}")),
            }
            busy = !project.running.lock().unwrap().is_empty();
        }
        statuses.push(ProjectStatus { error, entry, attention, busy });
    }
    let attention = Harness::ALL
        .iter()
        .map(|harness| host.db.domain(harness.as_str()))
        .filter(|domain| domain.as_ref().map_or(true, Domain::is_blocked))
        .map(|domain| Ok(Attention::Quota { domain: domain? }))
        .collect::<anyhow::Result<_>>()?;
    let snapshot = HostSnapshot { projects: statuses, attention, harnesses: harnesses(host)? };
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
