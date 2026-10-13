//! Observe the display repository without changing it; explicitly adopt one fixed commit (#67).
use std::path::Path;
use std::sync::Arc;

use lobotomy_core::display::{self, AdoptDisplay, Adoption, DisplayState, FinishAdoption, ObserveDisplay};
use lobotomy_core::project::{config_version, require_project};
use lobotomy_core::{Caller, Error};
use lobotomy_store::{Scope, Workspace, repo};

use crate::project::Project;
use crate::runner::{db, runtime};

/// Call under `activity`. A stale observation cannot overwrite a newer preview receipt.
pub async fn observe(project: &Arc<Project>) -> anyhow::Result<DisplayState> {
    let p = db(project, |db| db.read(require_project)).await?;
    let previous = p.previewed.clone();
    let state = tokio::task::spawn_blocking(move || match repo::inspect(Path::new(&p.repo_path)) {
        Err(e) => DisplayState::Blocked { reason: format!("无法读取展示仓库：{e}") },
        Ok(s) if s.branch.as_deref() != Some(&p.branch) => DisplayState::Blocked {
            reason: repo::Diverged::Branch { expected: p.branch, actual: s.branch }.to_string(),
        },
        Ok(s) if !s.dirty.is_empty() => DisplayState::Blocked {
            reason: format!("展示目录有未提交修改，请先在外部提交：{}", s.dirty.join("、")),
        },
        Ok(s) => match s.head {
            Some(commit) if commit == p.previewed => DisplayState::Current,
            Some(commit) => DisplayState::Changed { commit },
            None => DisplayState::Blocked { reason: "展示分支没有提交".into() },
        },
    })
    .await?;
    // Avoid a command log entry every idle poll when nothing changed.
    let seen = db(project, |db| db.read(display::observation)).await?;
    if seen != state {
        runtime(project, ObserveDisplay { previewed: previous, state: state.clone() }).await?;
    }
    Ok(state)
}

fn require_stopped(project: &Project, own_job: Option<&str>) -> anyhow::Result<()> {
    if !project.running.lock().unwrap().is_empty()
        || project.jobs.lock().unwrap().keys().any(|k| Some(k.as_str()) != own_job)
    {
        return Err(Error::rejected("not_at_savepoint", "等待执行者和物化、采集、检查、预览工作实际退出后接入").into());
    }
    Ok(())
}

/// User admission is serialized with scheduling. The intent then blocks all new work until CAS.
pub async fn begin(project: &Arc<Project>, cmd: AdoptDisplay) -> anyhow::Result<String> {
    let _gate = project.activity.lock().await;
    // A lost reply retries the original command even if its job has already started. Db::execute
    // still checks the command name and exact arguments before returning the recorded result.
    let request = cmd.request_id.clone();
    let recorded = db(project, move |db| {
        db.read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM command_record WHERE caller='user' AND idem_key=?1)",
                [request],
                |r| r.get::<_, bool>(0),
            )?)
        })
    })
    .await?;
    if recorded {
        return db(project, move |db| db.execute(&Caller::User, &cmd)).await;
    }
    observe(project).await?;
    require_stopped(project, None)?;
    let id = db(project, move |db| db.execute(&Caller::User, &cmd)).await?;
    project.wake.notify_one();
    Ok(id)
}

async fn recheck(project: &Arc<Project>, a: &Adoption, own_job: &str) -> anyhow::Result<()> {
    require_stopped(project, Some(own_job))?;
    let state = observe(project).await?;
    anyhow::ensure!(state == (DisplayState::Changed { commit: a.target.clone() }), "展示仓库已变化，请重新接入");
    let a = a.clone();
    db(project, move |db| {
        db.read(|c| {
            display::require_savepoint(c)?;
            let p = require_project(c)?;
            if p.integration != a.previous
                || p.integration_rev != a.expected_rev
                || lobotomy_core::project::current_config(c)?.0 != a.config_version
            {
                return Err(Error::rejected("stale_display", "内部版本或检查配置已变化，请重新接入"));
            }
            Ok(())
        })
    })
    .await
}

/// A restart repeats the same pinned target and checks, never a newly observed head. No display
/// files are written. Failure records evidence while leaving the old baseline and previews alone.
pub async fn adopt(project: Arc<Project>, id: String) -> anyhow::Result<()> {
    let aid = id.clone();
    let a = db(&project, move |db| db.read(|c| display::load(c, &aid))).await?;
    if a.state != "pending" {
        return Ok(());
    }
    let own_job = format!("adopt:{id}");
    let mut checks = Vec::new();
    let result = async {
        {
            let _gate = project.activity.lock().await;
            recheck(&project, &a, &own_job).await?;
        }
        let p = db(&project, |db| db.read(require_project)).await?;
        let (store, fixed) = (project.store.clone(), a.clone());
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            store.pin(&format!("{}-previous", fixed.id), &fixed.previous)?;
            let commit = match store.pinned(&fixed.id)? {
                Some(commit) => commit,
                None => store.import(Path::new(&p.repo_path), &p.branch, Some(&fixed.target), &fixed.id)?,
            };
            anyhow::ensure!(commit == fixed.target, "接入 pin 与固定目标不一致");
            anyhow::ensure!(store.conflicts(&commit)?.is_empty(), "展示提交包含未解决的冲突");
            Ok(())
        })
        .await??;
        let version = a.config_version;
        let config = db(&project, move |db| db.read(|c| config_version(c, version))).await?;
        let _site = project.verify_site.lock().await;
        let dir = project.data_dir.join("display-check");
        // This site belongs only to the fixed adoption; a restart gets a fresh site, without
        // touching worker slots, the display directory or another verification's caches.
        let (p, target, scope, check_dir) = (
            project.clone(),
            a.target.clone(),
            Scope { excluded: config.excluded.clone(), force_tracked: config.force_tracked.clone() },
            dir.clone(),
        );
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let state = p.data_dir.join("workspaces/display-check");
            if check_dir.exists() {
                std::fs::remove_dir_all(&check_dir)?;
            }
            if state.exists() {
                std::fs::remove_dir_all(&state)?;
            }
            Workspace::new(check_dir, state).materialize(&p.store, &target, &target, "display-check", &scope)?;
            Ok(())
        })
        .await??;
        {
            let _gate = project.activity.lock().await;
            recheck(&project, &a, &own_job).await?;
        }
        for check in &config.checks {
            let outcome = crate::results::run_check(&project, check, &dir).await?;
            let passed = outcome.passed();
            checks.push(outcome);
            if !passed {
                break;
            }
        }
        anyhow::Ok(())
    }
    .await;
    let _gate = project.activity.lock().await;
    let error = match result {
        Err(e) => Some(format!("{e:#}")),
        Ok(()) => recheck(&project, &a, &own_job).await.err().map(|e| format!("{e:#}")),
    };
    runtime(&project, FinishAdoption { id, checks, error }).await?;
    Ok(())
}
