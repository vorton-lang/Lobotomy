//! The Lobotomy backend process. v1 runs one project per instance (data-model.md §10).
//!
//! `lobotomyd --data-dir <dir> [--host-dir <dir>] [--port <n>]` runs the backend. The host
//! directory holds state shared across projects (data-model.md §10).
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
    host_dir: PathBuf,
    port: u16,
}

fn parse(args: &[String]) -> anyhow::Result<Options> {
    let (mut data_dir, mut host_dir, mut port) = (None, None, 0);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data-dir" => data_dir = it.next().map(PathBuf::from),
            "--host-dir" => host_dir = it.next().map(PathBuf::from),
            "--port" => port = it.next().context("--port needs a value")?.parse()?,
            other => bail!("unknown argument {other}"),
        }
    }
    let data_dir = data_dir.context("usage: lobotomyd --data-dir <dir> [--host-dir <dir>] [--port <n>]")?;
    let host_dir = match host_dir {
        Some(dir) => dir,
        None => Host::default_dir()?,
    };
    Ok(Options { data_dir, host_dir, port })
}

async fn serve(Options { data_dir, host_dir, port }: Options) -> anyhow::Result<()> {
    let host = Arc::new(Host::open(&host_dir, HarnessConfig::detect()?)?);
    let project = Arc::new(Project::open(&data_dir, host)?);
    let backend = Backend::start(project, port).await?;
    tracing::info!(data_dir = %data_dir.display(), addr = %backend.addr, "backend running");
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");
    backend.shutdown(SHUTDOWN_GRACE).await;
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
