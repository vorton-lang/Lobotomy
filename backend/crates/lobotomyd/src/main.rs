//! The Lobotomy backend process. v1 runs one project per instance (data-model.md §10).
//!
//! `lobotomyd --data-dir <dir> [--port <n>]` runs the backend.
//! `lobotomyd ctrl-c <pid>` is the helper that sends Ctrl+C to a CLI's console
//! (harness-adapter.md §1.8).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use lobotomyd::Backend;
use lobotomyd::project::{HarnessConfig, Project};
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
    let (data_dir, port) = parse(&args)?;
    lobotomy_harness::process::isolate_std_handles();
    tokio::runtime::Runtime::new()?.block_on(serve(data_dir, port))
}

fn parse(args: &[String]) -> anyhow::Result<(PathBuf, u16)> {
    let mut data_dir = None;
    let mut port = 0;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data-dir" => data_dir = it.next().map(PathBuf::from),
            "--port" => port = it.next().context("--port needs a value")?.parse()?,
            other => bail!("unknown argument {other}"),
        }
    }
    let data_dir = data_dir.context("usage: lobotomyd --data-dir <dir> [--port <n>]")?;
    Ok((data_dir, port))
}

async fn serve(data_dir: PathBuf, port: u16) -> anyhow::Result<()> {
    let project = Arc::new(Project::open(&data_dir, HarnessConfig::detect()?)?);
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
