//! User-started trials: a fresh copy of one fixed candidate and a separate native terminal.
//! Reporting a command never starts it. Only the authenticated GUI's explicit command does.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use lobotomy_core::Error;
use lobotomy_core::view::task_detail;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::project::Project;
use crate::{launch, runner};

#[derive(Clone, Serialize)]
pub struct TrialView {
    pub id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub round: i64,
    pub candidate: String,
    pub directory: String,
    pub command: String,
    pub purpose: String,
    pub state: &'static str,
    pub error: Option<String>,
}

struct Trial {
    view: TrialView,
    stop: CancellationToken,
    finished: CancellationToken,
}

#[derive(Default)]
pub struct Trials {
    entries: Mutex<Vec<Trial>>,
    starting: tokio::sync::Mutex<()>,
    closing: AtomicBool,
}

impl Trials {
    pub fn list(&self, task_id: &str) -> Vec<TrialView> {
        self.entries.lock().unwrap().iter().filter(|t| t.view.task_id == task_id).map(|t| t.view.clone()).collect()
    }

    pub async fn stop(&self, id: &str) -> anyhow::Result<()> {
        let (stop, finished) = {
            let trials = self.entries.lock().unwrap();
            let trial = trials.iter().find(|t| t.view.id == id).context("trial not found")?;
            (trial.stop.clone(), trial.finished.clone())
        };
        stop.cancel();
        finished.cancelled().await;
        Ok(())
    }

    /// Stops every trial and waits until their terminals have exited.
    pub async fn stop_all(&self) {
        self.closing.store(true, Ordering::SeqCst);
        let finished: Vec<_> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|trial| {
                trial.stop.cancel();
                trial.finished.clone()
            })
            .collect();
        for finished in finished {
            finished.cancelled().await;
        }
    }
}

#[derive(Deserialize)]
pub struct StartTrial {
    pub task_id: String,
    pub attempt_id: String,
    pub expected_candidate: String,
}

/// Reject a stale button before preparing anything, and use only the stored report's command.
async fn plan(project: &Arc<Project>, request: StartTrial) -> anyhow::Result<TrialView> {
    runner::db(project, move |db| {
        db.read(|c| {
            let detail = task_detail(c, &request.task_id)?;
            let attempt =
                detail.attempts.last().ok_or_else(|| Error::rejected("no_candidate", "本轮还没有候选成果"))?;
            if attempt.id != request.attempt_id {
                return Err(Error::rejected("stale_trial", "任务已进入新一轮，请查看本轮交付说明"));
            }
            let candidate = detail
                .captures
                .iter()
                .find(|c| Some(&c.id) == attempt.candidate_id.as_ref())
                .and_then(|c| c.commit_id.as_ref())
                .ok_or_else(|| Error::rejected("no_candidate", "本轮成果尚未固定"))?;
            if candidate != &request.expected_candidate {
                return Err(Error::rejected("stale_trial", "候选版本已变化，请刷新后再试用"));
            }
            let trial =
                attempt.trial.as_ref().ok_or_else(|| Error::rejected("no_trial", "执行者没有提供本轮试用命令"))?;
            if trial.command.trim().is_empty() || trial.purpose.trim().is_empty() {
                return Err(Error::rejected("no_trial", "试用命令和用途不能为空"));
            }
            Ok(TrialView {
                id: lobotomy_core::id::new_id("trial"),
                task_id: request.task_id,
                attempt_id: attempt.id.clone(),
                round: attempt.seq,
                candidate: candidate.clone(),
                directory: String::new(),
                command: trial.command.clone(),
                purpose: trial.purpose.clone(),
                state: "open",
                error: None,
            })
        })
    })
    .await
}

fn copy_root(project: &Project, trial_id: &str) -> PathBuf {
    project.trials_dir().join(trial_id)
}

/// Deletes a trial's copy. Processes just killed may hold it for a moment, so this tries for a few
/// seconds; a copy still in use after that goes at the next startup (results::sweep).
async fn discard(root: PathBuf) {
    const TRIES: u32 = 20;
    for tried in 1..=TRIES {
        let dir = root.clone();
        match tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir)).await {
            Ok(Err(e)) if e.kind() != std::io::ErrorKind::NotFound && tried == TRIES => {
                tracing::warn!(path = %root.display(), error = %e, "could not delete a trial copy; startup deletes it");
            }
            Ok(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            _ => return,
        }
    }
}

/// Materialize once in a new directory. Neither the worker slot nor the reusable verification
/// site is involved, and later candidates never overwrite this directory.
async fn materialize(project: &Arc<Project>, view: &mut TrialView) -> anyhow::Result<()> {
    let root = copy_root(project, &view.id);
    let directory = root.join("candidate");
    let state = root.join("state");
    let store = project.store.clone();
    let candidate = view.candidate.clone();
    view.directory = directory.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || {
        lobotomy_store::Workspace::new(directory, state).materialize(
            &store,
            &candidate,
            &candidate,
            "trial",
            &lobotomy_store::Scope::default(),
        )
    })
    .await??;
    Ok(())
}

pub async fn start(project: &Arc<Project>, request: StartTrial) -> anyhow::Result<TrialView> {
    let _starting = project.trials.starting.lock().await;
    let mut view = plan(project, request).await?;
    if let Some(open) =
        project.trials.list(&view.task_id).into_iter().find(|t| t.attempt_id == view.attempt_id && t.state == "open")
    {
        return Ok(open);
    }
    if project.trials.closing.load(Ordering::SeqCst) {
        anyhow::bail!("项目正在关闭");
    }
    if let Err(error) = launch(project, &mut view).await {
        discard(copy_root(project, &view.id)).await;
        return Err(error);
    }
    Ok(view)
}

/// Copies the candidate and opens its terminal. Once the terminal is open, the trial's owner
/// task deletes the copy when the terminal exits.
async fn launch(project: &Arc<Project>, view: &mut TrialView) -> anyhow::Result<()> {
    materialize(project, view).await?;
    if project.trials.closing.load(Ordering::SeqCst) {
        anyhow::bail!("项目正在关闭");
    }
    let title = format!("Lobotomy trial {} round {} {}", view.task_id, view.round, &view.candidate[..8]);
    let what =
        launch::What::Terminal { command: &view.command, title: &title, helper: &project.host.harness.terminal_helper };
    let mut spawned = launch::spawn(what, Path::new(&view.directory), &project.empty_gh_config_dir()).await?;
    let stop = CancellationToken::new();
    let finished = CancellationToken::new();
    {
        // Shutdown may have begun while the terminal opened. Never publish an unowned window.
        let mut entries = project.trials.entries.lock().unwrap();
        if project.trials.closing.load(Ordering::SeqCst) {
            let _ = spawned.child.start_kill();
            spawned.reap();
            anyhow::bail!("项目正在关闭");
        }
        if let Err(error) = spawned.resume() {
            let _ = spawned.child.start_kill();
            spawned.reap();
            return Err(error.into());
        }
        entries.push(Trial { view: view.clone(), stop: stop.clone(), finished: finished.clone() });
    }
    let (project, id, root) = (project.clone(), view.id.clone(), copy_root(project, &view.id));
    tokio::spawn(async move {
        let result = tokio::select! {
            result = spawned.child.wait() => result,
            _ = stop.cancelled() => {
                let _ = spawned.child.start_kill();
                spawned.child.wait().await
            }
        };
        spawned.reap();
        if let Some(trial) = project.trials.entries.lock().unwrap().iter_mut().find(|t| t.view.id == id) {
            trial.view.state = "closed";
            trial.view.error = result.err().map(|e| e.to_string());
        }
        finished.cancel();
        // After `finished`: neither Stop nor shutdown waits for the deletion.
        discard(root).await;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Cli, HarnessConfig, Host};
    use lobotomy_core::Caller;
    use lobotomy_core::capture::pending_captures;
    use lobotomy_core::report::{OrgReport, ReportStatus};
    use lobotomy_core::task::{Abandon, CreateTask, Reopen, StartAttempt, Trial};
    use lobotomy_core::turn::{EndTurn, Outcome, RegisterTurn, SessionIdentified};
    use lobotomy_core::workspace::materializing;

    async fn fixture() -> (tempfile::TempDir, Arc<Project>, String) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        for args in [
            vec!["init", "--quiet", "--initial-branch", "main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.com"],
        ] {
            assert!(std::process::Command::new("git").current_dir(&repo).args(args).status().unwrap().success());
        }
        std::fs::write(repo.join("version.txt"), "baseline").unwrap();
        for args in [vec!["add", "."], vec!["commit", "--quiet", "-m", "initial"]] {
            assert!(std::process::Command::new("git").current_dir(&repo).args(args).status().unwrap().success());
        }
        let cli = Cli { command: vec!["not-used".into()], reasoning_effort: None };
        let host = Arc::new(
            Host::open(
                &temp.path().join("host"),
                HarnessConfig {
                    claude: cli.clone(),
                    codex: cli,
                    interrupt_helper: vec![],
                    terminal_helper: PathBuf::new(),
                    probe_timeout: std::time::Duration::from_secs(1),
                },
            )
            .unwrap(),
        );
        let project = Arc::new(Project::open(&temp.path().join("project"), host).unwrap());
        crate::onboard::onboard(&project, &repo).await.unwrap();
        let task = project
            .db
            .execute(
                &Caller::User,
                &CreateTask {
                    request_id: "task".into(),
                    title: "trial".into(),
                    body: String::new(),
                    criteria: String::new(),
                    executor: "Malkuth".into(),
                },
            )
            .unwrap()
            .id;
        project.db.execute(&Caller::Runtime, &StartAttempt { task_id: task.clone(), code_start: None }).unwrap();
        for workspace in project.db.read(materializing).unwrap() {
            crate::results::materialize(project.clone(), workspace).await.unwrap();
        }
        let turn = project.db.execute(&Caller::Runtime, &RegisterTurn { role: "Malkuth".into() }).unwrap();
        project
            .db
            .execute(
                &Caller::Runtime,
                &SessionIdentified { turn_id: turn.turn_id.clone(), native_id: "native-trial".into() },
            )
            .unwrap();
        std::fs::write(project.slot_dir("worker").join("version.txt"), "candidate A").unwrap();
        project
            .db
            .execute(
                &Caller::Role { role: "Malkuth".into(), turn_id: turn.turn_id.clone() },
                &OrgReport {
                    title: "ready".into(),
                    body: String::new(),
                    status: ReportStatus::Done,
                    blocked_on: None,
                    trial: Some(Trial { command: "echo trial".into(), purpose: "see candidate".into() }),
                },
            )
            .unwrap();
        project
            .db
            .execute(&Caller::Runtime, &EndTurn { turn_id: turn.turn_id, outcome: Outcome::Completed, failure: None })
            .unwrap();
        for capture in project.db.read(pending_captures).unwrap() {
            crate::results::capture(project.clone(), capture).await.unwrap();
        }
        (temp, project, task)
    }

    fn request(project: &Project, task: &str) -> StartTrial {
        let detail = project.db.read(|c| task_detail(c, task)).unwrap();
        let attempt = detail.attempts.last().unwrap();
        let candidate = detail.captures.iter().find(|c| Some(&c.id) == attempt.candidate_id.as_ref()).unwrap();
        StartTrial {
            task_id: task.into(),
            attempt_id: attempt.id.clone(),
            expected_candidate: candidate.commit_id.clone().unwrap(),
        }
    }

    #[tokio::test]
    async fn trial_copy_is_exact_and_independent_of_worker_and_verification() {
        let (_temp, project, task) = fixture().await;
        // A report and capture alone create no trial and execute no command.
        assert!(project.trials.list(&task).is_empty());
        assert!(!project.data_dir.join("trials").exists());
        let mut first = plan(&project, request(&project, &task)).await.unwrap();
        materialize(&project, &mut first).await.unwrap();
        let first_file = std::path::Path::new(&first.directory).join("version.txt");
        assert_eq!(std::fs::read_to_string(&first_file).unwrap(), "candidate A");
        std::fs::write(project.slot_dir("worker").join("version.txt"), "uncommitted worker changes").unwrap();
        std::fs::create_dir_all(project.verify_dir()).unwrap();
        std::fs::write(project.verify_dir().join("version.txt"), "unrelated verification").unwrap();
        std::fs::write(&first_file, "trial-local change").unwrap();
        let mut second = plan(&project, request(&project, &task)).await.unwrap();
        materialize(&project, &mut second).await.unwrap();
        assert_ne!(first.directory, second.directory);
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(&second.directory).join("version.txt")).unwrap(),
            "candidate A"
        );
        assert_eq!(std::fs::read_to_string(first_file).unwrap(), "trial-local change");
        assert_eq!(
            std::fs::read_to_string(project.verify_dir().join("version.txt")).unwrap(),
            "unrelated verification"
        );
    }

    #[tokio::test]
    async fn trial_rejects_stale_commit_and_round_before_materializing() {
        let (_temp, project, task) = fixture().await;
        let mut wrong = request(&project, &task);
        wrong.expected_candidate = "wrong".into();
        assert!(plan(&project, wrong).await.is_err());
        let old = request(&project, &task);
        project
            .db
            .execute(
                &Caller::User,
                &Abandon { request_id: "abandon".into(), task_id: task.clone(), reason: String::new() },
            )
            .unwrap();
        project.db.execute(&Caller::User, &Reopen { request_id: "reopen".into(), task_id: task.clone() }).unwrap();
        let code_start = crate::results::code_start(&project, &task).await.unwrap();
        project.db.execute(&Caller::Runtime, &StartAttempt { task_id: task.clone(), code_start }).unwrap();
        assert!(plan(&project, old).await.is_err());
        assert!(!project.data_dir.join("trials").exists());
    }
}
