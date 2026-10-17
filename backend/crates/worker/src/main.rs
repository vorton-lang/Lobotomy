//! The worker host process. The orchestrator starts it and speaks the worker protocol on its stdin
//! and stdout (worker-interface.md §10); it logs to stderr.
//!
//! `lobotomy-worker --root <dir> --harness <claude|codex> [--model <m>] [--effort <e>]
//! [--permission <full|auto_review>] [--upstream <url|path> [--base <branch>] [--author-name <n>]
//! [--author-email <e>] [--port-start <n>]]` runs it. With `--upstream`, each attempt works in its
//! own clone of that repository; without it, in a plain folder. `LOBOTOMY_CODEX` and
//! `LOBOTOMY_CLAUDE` replace the located CLIs, as for the backend. `lobotomy-worker ctrl-c <pid>`
//! is the helper that sends Ctrl+C to a CLI's console (harness-adapter.md §1.8).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, bail};
use lobotomy_agent::HarnessConfig;
use lobotomy_harness::{Harness, Permission};
use lobotomy_worker::environment::Repo;
use lobotomy_worker::host::{Config, WorkerHost};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "usage: lobotomy-worker --root <dir> --harness <claude|codex> [--model <m>] [--effort <e>] \
    [--permission <full|auto_review>] [--upstream <url|path> [--base <branch>] [--author-name <n>] \
    [--author-email <e>] [--port-start <n>]]";

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
    let mut values = std::collections::HashMap::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let name = arg.strip_prefix("--").with_context(|| format!("unknown argument {arg}; {USAGE}"))?;
        let value = it.next().with_context(|| format!("{arg} needs a value; {USAGE}"))?;
        values.insert(name.to_owned(), value.clone());
    }
    let mut take = |name: &str| values.remove(name);
    let root = PathBuf::from(take("root").with_context(|| format!("--root is required; {USAGE}"))?);
    let harness = take("harness").with_context(|| format!("--harness is required; {USAGE}"))?;
    let harness = Harness::parse(&harness).with_context(|| format!("unknown harness {harness}"))?;
    let permission = match take("permission") {
        Some(name) => Permission::parse(&name).with_context(|| format!("unknown permission mode {name}"))?,
        None => Permission::default(),
    };
    let (model, effort) = (take("model"), take("effort"));
    let repo = match take("upstream") {
        Some(upstream) => Some(Repo {
            upstream,
            base: take("base").unwrap_or_else(|| "main".into()),
            author_name: take("author-name").unwrap_or_else(|| "Lobotomy Worker".into()),
            author_email: take("author-email").unwrap_or_else(|| "worker@lobotomy.invalid".into()),
            port_start: match take("port-start") {
                Some(n) => n.parse().with_context(|| format!("--port-start {n} is not a port"))?,
                None => 42000,
            },
        }),
        None => None,
    };
    if let Some(name) = values.keys().next() {
        bail!("unknown argument --{name}; {USAGE}");
    }
    std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    let config = Config { harness, cli: HarnessConfig::detect()?, model, effort, permission, repo };
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
