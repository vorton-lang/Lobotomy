//! The Lobotomy backend process. v1 runs one project per instance (data-model.md §10).
//!
//! `lobotomyd --data-dir <dir> [--repo <dir>] [--host-dir <dir>] [--port <n>]` runs the backend.
//! `--repo` connects the user's repository on the first run (harness-adapter.md §4.3), as the
//! GUI's onboarding also can; tests and scripts use it. The host directory holds state shared
//! across projects (data-model.md §10). One backend runs per data directory.
//! `lobotomyd ctrl-c <pid>` is the helper that sends Ctrl+C to a CLI's console
//! (harness-adapter.md §1.8).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use lobotomyd::Backend;
use lobotomyd::host::{HarnessConfig, Host};
use lobotomyd::project::Project;
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
    data_dir: PathBuf,
    repo: Option<PathBuf>,
    host_dir: PathBuf,
    port: u16,
}

fn parse(args: &[String]) -> anyhow::Result<Options> {
    let (mut data_dir, mut repo, mut host_dir, mut port) = (None, None, None, 0);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data-dir" => data_dir = it.next().map(PathBuf::from),
            "--repo" => repo = it.next().map(PathBuf::from),
            "--host-dir" => host_dir = it.next().map(PathBuf::from),
            "--port" => port = it.next().context("--port needs a value")?.parse()?,
            other => bail!("unknown argument {other}"),
        }
    }
    let data_dir =
        data_dir.context("usage: lobotomyd --data-dir <dir> [--repo <dir>] [--host-dir <dir>] [--port <n>]")?;
    let host_dir = match host_dir {
        Some(dir) => dir,
        None => Host::default_dir()?,
    };
    Ok(Options { data_dir, repo, host_dir, port })
}

async fn serve(Options { data_dir, repo, host_dir, port }: Options) -> anyhow::Result<()> {
    let host = Arc::new(Host::open(&host_dir, HarnessConfig::detect()?)?);
    let project = Arc::new(Project::open(&data_dir, host)?);
    let connected = project.db.read(lobotomy_core::project::load_project)?;
    match (connected, repo) {
        (None, Some(repo)) => lobotomyd::onboard::onboard(&project, &repo).await?,
        (None, None) => tracing::warn!("no repository connected yet; start once with --repo <dir>"),
        (Some(existing), Some(repo)) => tracing::warn!(
            connected = existing.repo_path,
            ignored = %repo.display(),
            "this project is already connected; --repo is ignored"
        ),
        (Some(_), None) => {}
    }
    let backend = Backend::start(project.clone(), port).await?;
    tracing::info!(data_dir = %data_dir.display(), addr = %backend.addr, "backend running");
    // How the GUI finds a running backend (frontend.md §1).
    let info = data_dir.join("backend.json");
    let contents = serde_json::json!({ "pid": std::process::id(), "port": backend.addr.port() });
    std::fs::write(&info, contents.to_string()).with_context(|| format!("writing {}", info.display()))?;
    tokio::select! {
        signal = tokio::signal::ctrl_c() => signal?,
        _ = project.shutdown_requested.notified() => {}
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
