//! Connecting the user's repository (harness-adapter.md §4.3 "接入"; data-model.md §9.2).

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use lobotomy_core::project::{Onboard, load_project};
use lobotomy_core::{Caller, Error, id::new_id};
use lobotomy_store::repo;

use crate::project::Project;
use crate::runner::db;

/// Checks the repository, copies its branch into the private store as the first integration
/// version, and records the project. The repository must be clean and on a branch; otherwise
/// nothing happens and the reasons are returned.
pub async fn onboard(project: &Arc<Project>, repo_path: &Path) -> anyhow::Result<()> {
    if let Some(existing) = db(project, |db| db.read(load_project)).await? {
        return Err(Error::rejected(
            "already_onboarded",
            format!("this project is already connected to {}", existing.repo_path),
        )
        .into());
    }
    let repo_path = self::repo_path(repo_path)?;
    let store = project.store.clone();
    let path = repo_path.clone();
    let (branch, head, author) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let state = repo::inspect(&path)?;
        let mut problems = Vec::new();
        if state.branch.is_none() {
            problems.push("HEAD 处于 detached 状态，需要检出在某个分支上".to_owned());
        }
        if state.head.is_none() {
            problems.push("仓库还没有提交".to_owned());
        }
        if !state.dirty.is_empty() {
            problems.push(format!("工作区不干净：{}", state.dirty.join("、")));
        }
        let author = repo::identity(&path)?;
        if author.is_none() {
            problems.push("没有配置 git 的 user.name 与 user.email".to_owned());
        }
        if !problems.is_empty() {
            return Err(Error::rejected(
                "repo_unusable",
                format!("不能接入 {}：{}", path.display(), problems.join("；")),
            )
            .into());
        }
        let (branch, head, author) = (state.branch.unwrap(), state.head.unwrap(), author.unwrap());
        store.import(&path, &branch, Some(&head), "import")?;
        Ok((branch, head, author))
    })
    .await??;
    let cmd = Onboard {
        request_id: new_id("req"),
        project_id: project.id.clone(),
        repo_path: repo_path.to_string_lossy().into_owned(),
        branch,
        head,
        author_name: author.name,
        author_email: author.email,
    };
    db(project, move |db| db.execute(&Caller::User, &cmd)).await?;
    project.wake.notify_one();
    Ok(())
}

/// The repository's path as the project records it: canonical, so one repository has one path
/// whichever way the user named it (data-model.md §10.3).
pub fn repo_path(path: &Path) -> anyhow::Result<std::path::PathBuf> {
    let canonical = std::fs::canonicalize(path).with_context(|| format!("opening {}", path.display()))?;
    Ok(dunce(&canonical))
}

/// `canonicalize` on Windows gives a `\\?\` path, which git does not take everywhere.
fn dunce(path: &Path) -> std::path::PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => rest.into(),
        _ => path.to_path_buf(),
    }
}
