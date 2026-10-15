//! Observe the display repository without changing it; explicitly adopt one fixed commit (#67).
use std::path::Path;
use std::sync::Arc;

use lobotomy_core::display::{self, AdoptDisplay, Adoption, DisplayState, FinishAdoption, ObserveDisplay};
use lobotomy_core::project::{config_version, require_project};
use lobotomy_core::{Caller, Error};
use lobotomy_store::repo;

use crate::project::Project;
use crate::runner::{db, runtime};

/// Call under `activity` to serialize observation with adoption and new starts.
pub async fn observe(project: &Arc<Project>) -> anyhow::Result<DisplayState> {
    let p = db(project, |db| db.read(require_project)).await?;
    let state = tokio::task::spawn_blocking(move || match repo::inspect(Path::new(&p.repo_path)) {
        Err(e) => DisplayState::Blocked { commit: None, reason: format!("无法读取展示目录：{e}") },
        Ok(s) => {
            let reason = if s.branch.as_deref() != Some(&p.branch) {
                Some(repo::Diverged::Branch { expected: p.branch, actual: s.branch }.to_string())
            } else if !s.dirty.is_empty() {
                Some(format!("展示目录有未提交修改，请先在外部提交：{}", s.dirty.join("、")))
            } else if s.head.is_none() {
                Some("展示分支没有提交".into())
            } else {
                None
            };
            match (s.head, reason) {
                (commit, Some(reason)) => DisplayState::Blocked { commit, reason },
                (Some(commit), None) if commit != p.previewed => DisplayState::Changed { commit },
                _ => DisplayState::Current,
            }
        }
    })
    .await?;
    // Avoid a command log entry every idle poll when nothing changed.
    let seen = db(project, |db| db.read(display::observation)).await?;
    if seen != state {
        runtime(project, ObserveDisplay { state: state.clone() }).await?;
    }
    Ok(state)
}

fn require_stopped(project: &Project, own_job: &str) -> anyhow::Result<()> {
    if !project.running.lock().unwrap().is_empty() || project.jobs.lock().unwrap().keys().any(|k| k != own_job) {
        return Err(Error::rejected("not_at_savepoint", "等待执行者和物化、采集、检查、预览工作实际退出后采用").into());
    }
    Ok(())
}

/// User admission is serialized with scheduling. The intent then blocks all new work until CAS.
pub async fn begin(project: &Arc<Project>, cmd: AdoptDisplay) -> anyhow::Result<String> {
    let _gate = project.activity.lock().await;
    observe(project).await?;
    let id = db(project, move |db| db.execute(&Caller::User, &cmd)).await?;
    project.wake.notify_one();
    Ok(id)
}

async fn recheck(project: &Arc<Project>, a: &Adoption, own_job: &str) -> anyhow::Result<()> {
    require_stopped(project, own_job)?;
    let state = observe(project).await?;
    anyhow::ensure!(state == (DisplayState::Changed { commit: a.target.clone() }), "展示目录已变化，请重新采用");
    Ok(())
}

/// Checks one fixed target without writing display files. Restart recovery fails the intent
/// and waits for a new user request; pins and historical evidence remain available.
pub async fn adopt(project: Arc<Project>, id: String) -> anyhow::Result<()> {
    let aid = id.clone();
    let a = db(&project, move |db| db.read(|c| display::load(c, &aid))).await?;
    if a.state != "pending" {
        return Ok(());
    }
    let own_job = format!("adopt:{id}");
    let mut checks = Vec::new();
    let mut blobs = Vec::new();
    let result = async {
        {
            let _gate = project.activity.lock().await;
            recheck(&project, &a, &own_job).await?;
        }
        let p = db(&project, |db| db.read(require_project)).await?;
        let branch = p.branch.clone();
        let (store, fixed) = (project.store.clone(), a.clone());
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            store.pin(&format!("{}-previous", fixed.id), &fixed.previous)?;
            let commit = match store.pinned(&fixed.id)? {
                Some(commit) => commit,
                None => store.import(Path::new(&p.repo_path), &p.branch, Some(&fixed.target), &fixed.id)?,
            };
            anyhow::ensure!(commit == fixed.target, "采用 pin 与固定目标不一致");
            Ok(())
        })
        .await??;
        let version = a.config_version;
        let config = db(&project, move |db| db.read(|c| config_version(c, version))).await?;
        let _site = project.verify_site.lock().await;
        crate::results::write_verify_site(&project, &a.target, &a.target, &branch, &config).await?;
        {
            let _gate = project.activity.lock().await;
            recheck(&project, &a, &own_job).await?;
        }
        (checks, blobs) = crate::results::run_checks(&project, &config.checks, || async { Ok(true) }).await?;
        anyhow::Ok(())
    }
    .await;
    let _gate = project.activity.lock().await;
    let error = match result {
        Err(e) => Some(format!("{e:#}")),
        Ok(()) => recheck(&project, &a, &own_job).await.err().map(|e| format!("{e:#}")),
    };
    runtime(&project, FinishAdoption { id, checks, blobs, error }).await?;
    Ok(())
}
