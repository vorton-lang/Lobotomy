//! The worker host process. The orchestrator starts it and speaks the worker protocol on its stdin
//! and stdout (worker-interface.md §10); it logs to stderr.
//!
//! `lobotomy-worker --root <dir> --harness <claude|codex> [--model <m>] [--effort <e>]
//! [--permission <full|auto_review>]` runs it. `LOBOTOMY_CODEX` and `LOBOTOMY_CLAUDE` replace the
//! located CLIs, as for the backend. `lobotomy-worker ctrl-c <pid>` is the helper that sends Ctrl+C
//! to a CLI's console (harness-adapter.md §1.8).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, bail};
use lobotomy_agent::HarnessConfig;
use lobotomy_harness::{Harness, Permission};
use lobotomy_worker::host::{Config, WorkerHost};
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [command, pid] = args.as_slice()
        && command == "ctrl-c"
    {
        return send_ctrl_c(pid);
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();
    let (root, config) = parse(&args)?;
    // The protocol pipes must not reach the CLIs: one holding stdout open would keep the
    // orchestrator from seeing this process end.
    lobotomy_harness::process::isolate_std_handles();
    tokio::runtime::Runtime::new()?.block_on(async {
        // One worker host per root. The OS releases the lock when the process ends.
        let _lock = lobotomy_agent::instance::lock_file(&root.join("lock"), &root.display().to_string())?;
        let host = Arc::new(WorkerHost::open(root, config)?);
        let input = tokio::io::BufReader::new(tokio::io::stdin());
        lobotomy_worker::run(host, input, tokio::io::stdout()).await
    })
}

fn parse(args: &[String]) -> anyhow::Result<(PathBuf, Config)> {
    let usage = "usage: lobotomy-worker --root <dir> --harness <claude|codex> [--model <m>] [--effort <e>] [--permission <full|auto_review>]";
    let (mut root, mut harness, mut model, mut effort, mut permission) =
        (None, None, None, None, Permission::default());
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().with_context(|| format!("{arg} needs a value; {usage}"));
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(value()?)),
            "--harness" => {
                let name = value()?;
                harness = Some(Harness::parse(name).with_context(|| format!("unknown harness {name}"))?);
            }
            "--model" => model = Some(value()?.clone()),
            "--effort" => effort = Some(value()?.clone()),
            "--permission" => {
                let name = value()?;
                permission = Permission::parse(name).with_context(|| format!("unknown permission mode {name}"))?;
            }
            other => bail!("unknown argument {other}; {usage}"),
        }
    }
    let root = root.with_context(|| format!("--root is required; {usage}"))?;
    let harness = harness.with_context(|| format!("--harness is required; {usage}"))?;
    std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    let config = Config { harness, cli: HarnessConfig::detect()?, model, effort, permission };
    Ok((root, config))
}

#[cfg(windows)]
fn send_ctrl_c(pid: &str) -> anyhow::Result<()> {
    Ok(lobotomy_harness::process::send_ctrl_c(pid.parse()?)?)
}

#[cfg(not(windows))]
fn send_ctrl_c(_pid: &str) -> anyhow::Result<()> {
    bail!("the ctrl-c helper is only needed on Windows")
}
