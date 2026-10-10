//! The Lobotomy backend process: one per host directory, for all its projects (data-model.md
//! §10).
//!
//! `lobotomyd [--host-dir <dir>] [--port <n>] [--repo <dir>]` runs the backend. `--repo` adds the
//! repository as a project unless it already has one (harness-adapter.md §4.3), as the GUI's new
//! project also does; tests and scripts use it.
//! `lobotomyd ctrl-c <pid>` is the helper that sends Ctrl+C to a CLI's console
//! (harness-adapter.md §1.8).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use lobotomy_core::host::ProjectState;
use lobotomyd::Backend;
use lobotomyd::host::{HarnessConfig, Host};
use tracing_subscriber::EnvFilter;

/// How long a normal shutdown waits for interrupted CLIs to exit.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [mode, title, command, control] = args.as_slice()
        && mode == "trial-terminal"
    {
        std::process::exit(lobotomy_harness::process::run_terminal_helper(
            command,
            title,
            std::path::Path::new(control),
        )?);
    }
    if let [command, pid] = args.as_slice()
        && command == "ctrl-c"
    {
        return send_ctrl_c(pid);
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let options = parse(&args)?;
    lobotomy_harness::process::isolate_std_handles();
    tokio::runtime::Runtime::new()?.block_on(serve(options))
}

struct Options {
    host_dir: PathBuf,
    repo: Option<PathBuf>,
    port: u16,
}

fn parse(args: &[String]) -> anyhow::Result<Options> {
    let (mut repo, mut host_dir, mut port) = (None, None, 0);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--repo" => repo = it.next().map(PathBuf::from),
            "--host-dir" => host_dir = it.next().map(PathBuf::from),
            "--port" => port = it.next().context("--port needs a value")?.parse()?,
            other => bail!("unknown argument {other}; usage: lobotomyd [--host-dir <dir>] [--port <n>] [--repo <dir>]"),
        }
    }
    let host_dir = match host_dir {
        Some(dir) => dir,
        None => Host::default_dir()?,
    };
    Ok(Options { host_dir, repo, port })
}

async fn serve(Options { host_dir, repo, port }: Options) -> anyhow::Result<()> {
    std::fs::create_dir_all(&host_dir).with_context(|| format!("creating {}", host_dir.display()))?;
    // One backend per host directory (data-model.md §10.3). The OS releases the lock when the
    // process ends, however it ends.
    let _lock = lobotomyd::project::lock_file(&host_dir.join("lock"), &host_dir.display().to_string())?;
    let host = Arc::new(Host::open(&host_dir, HarnessConfig::detect()?)?);
    let backend = Backend::start(host.clone(), port).await?;
    if let Some(repo) = repo {
        let path = lobotomyd::onboard::repo_path(&repo)?.to_string_lossy().into_owned();
        let existing =
            host.db.projects()?.into_iter().find(|p| p.repo_path == path && p.state != ProjectState::Archived);
        match existing {
            Some(project) => tracing::info!(project = project.id, repo = path, "the repository already has a project"),
            None => {
                let project = backend.projects.create(&repo).await?;
                tracing::info!(project = project.id, repo = path, "added the repository as a project");
            }
        }
    }
    tracing::info!(host_dir = %host_dir.display(), addr = %backend.addr, "backend running");
    // How the GUI finds a running backend (frontend.md §1).
    let info = host_dir.join("backend.json");
    let contents = serde_json::json!({ "pid": std::process::id(), "port": backend.addr.port() });
    std::fs::write(&info, contents.to_string()).with_context(|| format!("writing {}", info.display()))?;
    tokio::select! {
        signal = tokio::signal::ctrl_c() => signal?,
        _ = host.shutdown_requested.notified() => {}
    }
    tracing::info!("shutting down");
    backend.shutdown(SHUTDOWN_GRACE).await;
    let _ = std::fs::remove_file(&info);
    Ok(())
}

#[cfg(windows)]
fn send_ctrl_c(pid: &str) -> anyhow::Result<()> {
    Ok(lobotomy_harness::process::send_ctrl_c(pid.parse()?)?)
}

#[cfg(not(windows))]
fn send_ctrl_c(_pid: &str) -> anyhow::Result<()> {
    bail!("the ctrl-c helper is only needed on Windows")
}
