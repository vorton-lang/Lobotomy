//! Store work the scheduler hands out: writing slots, capturing turns, verifying candidates and
//! writing the preview (harness-adapter.md §3, §4; data-model.md §5, §6).
//!
//! Each job reads its intent from the database, does the side effect, and records the result
//! with a runtime command, whose preconditions decide whether the result still counts.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use lobotomy_core::capture::{Capture, CaptureResult, FinishCapture};
use lobotomy_core::id::new_id;
use lobotomy_core::item::externalize;
use lobotomy_core::project::{Check, ProjectConfig, config_version, current_config, require_project};
use lobotomy_core::task::CodeStart;
use lobotomy_core::verify::{
    CheckOutcome, FinishPreview, FinishVerification, PreviewJob, PreviewResult, verification_plan, work_so_far,
};
use lobotomy_core::workspace::{ReplaceWorkspace, Workspace, WorkspaceReady, current_workspace, load_workspace};
use lobotomy_harness::process::{self, Spec};
use lobotomy_store::{Captured, Guard, Identity, Leave, Scope, repo};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::capability;
use crate::project::Project;
use crate::runner::{db, runtime};

/// How much of a check's stdout and stderr is kept, each. The end is kept: that is where test
/// runners print the failures and the summary.
const OUTPUT_CAP: usize = 4 * 1024 * 1024;

/// Lines of check output quoted back to the executor.
const TAIL_LINES: usize = 40;

/// Starts a job unless one with the same key is running or has failed.
///
/// A failed job stops with its reason and is not started again until the user retries
/// ([`retry_failed`]); a backend restart runs it once more (data-model.md §5).
pub fn spawn_job<F>(project: &Arc<Project>, key: String, job: F)
where
    F: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    if project.failed.lock().unwrap().contains_key(&key) || !project.jobs.lock().unwrap().insert(key.clone()) {
        return;
    }
    let project = project.clone();
    tokio::spawn(async move {
        if let Err(e) = job.await {
            let reason = format!("{e:#}");
            tracing::error!(key, error = reason, "store job failed; waiting for the user to retry");
            project.failed.lock().unwrap().insert(key.clone(), reason);
        }
        project.jobs.lock().unwrap().remove(&key);
        project.wake.notify_one();
    });
}

/// Jobs that failed and why, by key: `slot:<name>`, `capture:<id>`, `verify:<id>`, `preview`.
pub fn failures(project: &Project) -> Vec<(String, String)> {
    let mut failed: Vec<_> = project.failed.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    failed.sort();
    failed
}

/// The user's retry: failed jobs may run again. Returns how many there were.
pub fn retry_failed(project: &Project) -> usize {
    let count = std::mem::take(&mut *project.failed.lock().unwrap()).len();
    project.wake.notify_one();
    count
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> anyhow::Result<T> {
    Ok(tokio::task::spawn_blocking(f).await?)
}

fn scope(config: &ProjectConfig) -> Scope {
    Scope { excluded: config.excluded.clone(), force_tracked: config.force_tracked.clone() }
}

fn workspace(project: &Project, ws: &Workspace) -> lobotomy_store::Workspace {
    lobotomy_store::Workspace::new(project.slot_dir(&ws.name), project.slot_state_dir(&ws.name, ws.generation))
}

/// Moves a directory aside; it is deleted once its replacement is ready, or at the next start.
fn quarantine(project: &Project, dir: &Path, label: &str) -> anyhow::Result<std::path::PathBuf> {
    let aside = project.quarantine_dir().join(format!("{label}-{}", new_id("q")));
    std::fs::create_dir_all(project.quarantine_dir())?;
    std::fs::rename(dir, &aside).with_context(|| format!("moving {} aside", dir.display()))?;
    Ok(aside)
}

/// Writes a slot's target (harness-adapter.md §3). A directory that cannot be brought to its
/// target is moved aside and the next generation starts empty.
pub async fn materialize(project: Arc<Project>, ws: Workspace) -> anyhow::Result<()> {
    let (branch, config) =
        db(&project, |db| db.read(|c| Ok((require_project(c)?.branch, current_config(c)?.1)))).await?;
    let scope = scope(&config);
    let write = |project: Arc<Project>, ws: Workspace, branch: String, scope: Scope| {
        blocking(move || {
            let store = &project.store;
            workspace(&project, &ws).materialize(store, &ws.target, &ws.head, &branch, &scope)
        })
    };
    let ws = match write(project.clone(), ws.clone(), branch.clone(), scope.clone()).await? {
        Ok(()) => ws,
        Err(lobotomy_store::Error::Blocked(files)) => {
            let reason = format!("{files} 个文件被未跟踪的文件挡住");
            tracing::warn!(slot = ws.name, generation = ws.generation, reason, "replacing the slot");
            let aside = quarantine(&project, &project.slot_dir(&ws.name), &format!("{}-g{}", ws.name, ws.generation))?;
            let old_state = project.slot_state_dir(&ws.name, ws.generation);
            let next = runtime(&project, ReplaceWorkspace { workspace_id: ws.id.clone(), reason }).await?;
            write(project.clone(), next.clone(), branch, scope).await??;
            let _ = std::fs::remove_dir_all(aside);
            let _ = std::fs::remove_dir_all(old_state);
            next
        }
        Err(e) => return Err(e.into()),
    };
    let ready = WorkspaceReady { workspace_id: ws.id, target: ws.target, head: ws.head };
    // A newer target came in while writing: the next round writes that one.
    crate::scheduler::ignore_rejection(runtime(&project, ready).await)?;
    Ok(())
}

/// Captures a turn's scene once its CLI has exited (harness-adapter.md §4.1 step ②) and
/// publishes the result (step ③).
pub async fn capture(project: Arc<Project>, capture: Capture) -> anyhow::Result<()> {
    let (workspace_id, version) = (capture.workspace_id.clone(), capture.config_version);
    let (ws, config) =
        db(&project, move |db| db.read(|c| Ok((load_workspace(c, &workspace_id)?, config_version(c, version)?))))
            .await?;
    let guard = (!capture.options.ignore_guard)
        .then_some(Guard { max_new_files: config.max_new_files, max_new_bytes: config.max_new_bytes });
    let leave = Leave { new_files: capture.options.leave_new_files, uncovered: capture.options.leave_uncovered };
    let p = project.clone();
    let c = capture.clone();
    let captured =
        blocking(move || workspace(&p, &ws).capture(&p.store, &c.base, &scope(&config), guard.as_ref(), leave, &c.id))
            .await??;
    let result = match captured {
        // What a turn outside any task changed waits for the user (#14).
        Captured::Pinned { commit } if capture.task_id.is_none() => {
            let (p, base, c) = (project.clone(), capture.base.clone(), commit.clone());
            let changed = blocking(move || p.store.changed_paths(&base, &c)).await??;
            CaptureResult::Pinned { commit, changed }
        }
        Captured::Pinned { commit } => CaptureResult::Pinned { commit, changed: vec![] },
        Captured::Oversized { files, total_bytes } => {
            CaptureResult::Oversized { files: serde_json::from_value(serde_json::to_value(files)?)?, total_bytes }
        }
        Captured::Uncovered { paths } => {
            CaptureResult::Uncovered { paths: serde_json::from_value(serde_json::to_value(paths)?)? }
        }
    };
    runtime(&project, FinishCapture { capture_id: capture.id, result }).await?;
    Ok(())
}

/// Rebases the candidate onto the integration version, writes the result into the verification
/// site and runs the checks there (harness-adapter.md §4.2; data-model.md §5).
pub async fn verify(project: Arc<Project>, verification_id: String) -> anyhow::Result<()> {
    let id = verification_id.clone();
    let plan = db(&project, move |db| db.read(|c| verification_plan(c, &id))).await?;
    let author = Identity { name: plan.author_name.clone(), email: plan.author_email.clone() };
    let (p, pl, pin) = (project.clone(), plan.clone(), verification_id.clone());
    let composed = blocking(move || p.store.compose(&pl.candidate, &pl.base, &pl.message, &author, &pin)).await??;

    let mut checks = Vec::new();
    // One verification at a time owns the site, from writing it until the processes of its last
    // check have ended: a check of an abandoned task that is still running must not write into
    // the next task's site (#12).
    let site = if composed.conflicts.is_empty() { Some(project.verify_site.lock().await) } else { None };
    if site.is_some() && still_verifying(&project, &verification_id).await? {
        let (p, pl, commit) = (project.clone(), plan.clone(), composed.commit.clone());
        blocking(move || -> anyhow::Result<()> {
            let site = lobotomy_store::Workspace::new(p.verify_dir(), p.verify_state_dir());
            let write = || site.materialize(&p.store, &commit, &pl.base, &pl.branch, &scope(&pl.config));
            match write() {
                // Nothing in the verification site is worth keeping; it starts over empty.
                Err(lobotomy_store::Error::Blocked(_)) => {
                    let aside = quarantine(&p, &p.verify_dir(), "verify")?;
                    let _ = std::fs::remove_dir_all(p.verify_state_dir());
                    write()?;
                    let _ = std::fs::remove_dir_all(aside);
                    Ok(())
                }
                other => Ok(other?),
            }
        })
        .await??;
        for check in &plan.config.checks {
            // A task that left verification, such as one abandoned, starts no further checks.
            // The checks that did not run keep the verification from passing.
            if !still_verifying(&project, &verification_id).await? {
                break;
            }
            let outcome = run_check(&project, check, &project.verify_dir()).await?;
            let passed = outcome.passed();
            checks.push(outcome);
            if !passed {
                break;
            }
        }
    }
    drop(site);
    let p = project.clone();
    let (checks, blobs) = blocking(move || {
        let mut blobs = Vec::new();
        let checks: Vec<CheckOutcome> = checks
            .into_iter()
            .map(|mut c| {
                blobs.extend(externalize(&p.blobs, &mut c.output));
                c
            })
            .collect();
        (checks, blobs)
    })
    .await?;
    let finish =
        FinishVerification { verification_id, commit: composed.commit, conflicts: composed.conflicts, checks, blobs };
    runtime(&project, finish).await?;
    Ok(())
}

/// Whether the verification's task still waits for it.
async fn still_verifying(project: &Arc<Project>, verification_id: &str) -> anyhow::Result<bool> {
    let id = verification_id.to_owned();
    db(project, move |db| {
        db.read(|c| {
            let v = lobotomy_core::verify::load_verification(c, &id)?;
            Ok(lobotomy_core::task::load_task(c, &v.task_id)?.phase == lobotomy_core::task::Phase::Verifying)
        })
    })
    .await
}

/// Runs one check command in the verification site, with the same environment trimming as a
/// role (harness-adapter.md §1.6). A check that runs past its timeout is ended with everything it
/// started.
async fn run_check(project: &Arc<Project>, check: &Check, cwd: &Path) -> anyhow::Result<CheckOutcome> {
    let gh = project.empty_gh_config_dir();
    std::fs::create_dir_all(&gh)?;
    let env = capability::env(&gh);
    let spec = Spec { program: Path::new(""), args: &[], cwd, env: &env, env_remove: capability::REMOVED_VARS };
    let started = Instant::now();
    let mut spawned = process::spawn_shell(&check.command, &spec)?;
    spawned.resume()?;
    drop(spawned.child.stdin.take());
    let stdout = tokio::spawn(read_capped(spawned.child.stdout.take().context("no stdout")?));
    let stderr = tokio::spawn(read_capped(spawned.child.stderr.take().context("no stderr")?));
    let waited = tokio::time::timeout(Duration::from_secs(check.timeout_secs), spawned.child.wait()).await;
    let (exit_code, timed_out) = match waited {
        Ok(status) => (status?.code().map(i64::from), false),
        Err(_) => {
            let _ = spawned.child.start_kill();
            let _ = spawned.child.wait().await;
            (None, true)
        }
    };
    // Ends whatever the command left running, which also closes the pipes.
    spawned.reap();
    let stdout = stdout.await?;
    let stderr = stderr.await?;
    let combined =
        [stdout.as_str(), stderr.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n");
    let lines: Vec<&str> = combined.lines().collect();
    let tail = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    Ok(CheckOutcome {
        command: check.command.clone(),
        exit_code,
        timed_out,
        output: json!({ "stdout": stdout, "stderr": stderr }),
        duration_ms: started.elapsed().as_millis() as i64,
        tail,
    })
}

/// Reads a stream to its end, keeping the last [`OUTPUT_CAP`] bytes.
async fn read_capped(mut stream: impl AsyncRead + Unpin) -> String {
    let (mut kept, mut buf, mut dropped) = (Vec::new(), vec![0u8; 64 * 1024], 0usize);
    while let Ok(n) = stream.read(&mut buf).await {
        if n == 0 {
            break;
        }
        kept.extend_from_slice(&buf[..n]);
        if kept.len() > OUTPUT_CAP {
            let excess = kept.len() - OUTPUT_CAP;
            kept.drain(..excess);
            dropped += excess;
        }
    }
    let text = String::from_utf8_lossy(&kept).into_owned();
    if dropped > 0 { format!("（前 {dropped} 字节已省略）\n{text}") } else { text }
}

/// Brings the user's repository to the latest integration version, if it is as the last preview
/// left it (harness-adapter.md §4.3).
pub async fn preview(project: Arc<Project>, job: PreviewJob) -> anyhow::Result<()> {
    let p = project.clone();
    let j = job.clone();
    let written =
        blocking(move || repo::fast_forward(Path::new(&j.repo_path), &p.store, &j.branch, &j.previewed, &j.target))
            .await?;
    let result = match written {
        Ok(None) => PreviewResult::Written,
        Ok(Some(diverged)) => PreviewResult::Stopped { reason: diverged.to_string() },
        Err(e) => PreviewResult::Stopped { reason: format!("写入预览时出错：{e}") },
    };
    if let PreviewResult::Stopped { reason } = &result {
        tracing::warn!(target = job.target, reason, "preview stopped");
    }
    runtime(&project, FinishPreview { outbox_id: job.outbox_id, result }).await?;
    Ok(())
}

/// Where a task's next attempt starts when it has work already: its last candidate, or the changes
/// it was made from, rebased onto the integration version (data-model.md §4.6). `None` for a task
/// without either.
pub async fn code_start(project: &Arc<Project>, task_id: &str) -> anyhow::Result<Option<CodeStart>> {
    let id = task_id.to_owned();
    let found = db(project, move |db| {
        db.read(|c| match work_so_far(c, &id)? {
            Some(candidate) => Ok(Some((candidate, require_project(c)?.integration))),
            None => Ok(None),
        })
    })
    .await?;
    let Some((candidate, integration)) = found else {
        return Ok(None);
    };
    // The pin names every input, so the same inputs give the same commit and a newer candidate a
    // new one (#12).
    let pin = format!("start-{task_id}-{candidate}-{integration}");
    let (p, base) = (project.clone(), integration.clone());
    let composed =
        blocking(move || p.store.compose(&candidate, &base, "code start", &Identity::runtime(), &pin)).await??;
    Ok(Some(CodeStart { commit: composed.commit, base: integration, conflicts: composed.conflicts }))
}

/// At startup, deletes what nothing owns any more: directories moved aside, and the jj state of
/// slot generations that were replaced (data-model.md §6).
pub async fn sweep(project: &Arc<Project>) -> anyhow::Result<()> {
    if let Ok(entries) = std::fs::read_dir(project.quarantine_dir()) {
        for entry in entries.flatten() {
            if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                tracing::warn!(path = %entry.path().display(), error = %e, "could not delete a quarantined directory");
            }
        }
    }
    let Ok(slots) = std::fs::read_dir(project.data_dir.join("workspaces")) else {
        return Ok(());
    };
    for slot in slots.flatten() {
        let name = slot.file_name().to_string_lossy().into_owned();
        if name == "verify" {
            continue;
        }
        let n = name.clone();
        let current = db(project, move |db| db.read(|c| current_workspace(c, &n))).await?.map(|ws| ws.generation);
        for generation in std::fs::read_dir(slot.path())?.flatten() {
            let keep = current.is_some_and(|g| generation.file_name().to_string_lossy() == format!("g{g}"));
            if !keep {
                let _ = std::fs::remove_dir_all(generation.path());
            }
        }
    }
    Ok(())
}
