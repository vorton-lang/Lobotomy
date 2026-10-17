//! How to start the harness CLIs, and the runs that are not turns: Codex's model catalog and the
//! quota check (harness-adapter.md §1.2; data-model.md §8.4).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use lobotomy_core::quota::CheckOutcome;
use lobotomy_harness::event::Event;
use lobotomy_harness::{Harness, Model, Output, Permission, claude, codex};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use crate::launch;

/// How to start the harness CLIs. Binary detection is host state (data-model.md §10).
#[derive(Clone, Debug)]
pub struct HarnessConfig {
    pub claude: Cli,
    pub codex: Cli,
    /// The program and arguments that send Ctrl+C to a pid (harness-adapter.md §1.8).
    pub interrupt_helper: Vec<String>,
    /// A quota check that has not finished by then counts as rejected without a reset time; the
    /// domain stays blocked (data-model.md §8.4). A normal check takes seconds.
    pub probe_timeout: Duration,
}

/// How to start one harness's CLI.
#[derive(Clone, Debug)]
pub struct Cli {
    /// The program and any arguments that go before the adapter's own. Tests put a fake CLI here.
    pub command: Vec<String>,
    /// The effort of roles that have none chosen. Only tests set it, to keep real turns cheap.
    pub reasoning_effort: Option<String>,
}

impl Cli {
    /// The program, and the arguments that go before the adapter's own.
    pub fn program(&self) -> anyhow::Result<(&Path, &[String])> {
        let (program, prefix) = self.command.split_first().context("no program configured")?;
        Ok((Path::new(program), prefix))
    }
}

impl HarnessConfig {
    /// The real CLIs, with this executable as the interrupt helper.
    /// `LOBOTOMY_CODEX` and `LOBOTOMY_CLAUDE`, each a JSON array of program and arguments, replace
    /// the located CLIs. The GUI's end-to-end tests put the fake CLIs there.
    pub fn detect() -> anyhow::Result<Self> {
        let exe = std::env::current_exe().context("locating the backend executable")?;
        let command = |var: &str, located: PathBuf| -> anyhow::Result<Vec<String>> {
            match std::env::var(var) {
                Ok(json) => {
                    serde_json::from_str(&json).with_context(|| format!("{var} is not a JSON array of strings"))
                }
                Err(_) => Ok(vec![located.to_string_lossy().into_owned()]),
            }
        };
        Ok(Self {
            claude: Cli { command: command("LOBOTOMY_CLAUDE", claude::locate())?, reasoning_effort: None },
            codex: Cli { command: command("LOBOTOMY_CODEX", codex::locate())?, reasoning_effort: None },
            interrupt_helper: vec![exe.to_string_lossy().into_owned(), "ctrl-c".into()],
            probe_timeout: Duration::from_secs(120),
        })
    }

    pub fn cli(&self, harness: Harness) -> &Cli {
        match harness {
            Harness::Claude => &self.claude,
            Harness::Codex => &self.codex,
        }
    }

    /// Reads Codex's model catalog. It calls no model. `dir` holds the directories it runs in.
    pub async fn codex_catalog(&self, dir: &Path) -> anyhow::Result<Vec<Model>> {
        let (program, prefix) = self.codex.program()?;
        let args: Vec<String> = prefix.iter().cloned().chain(codex::CATALOG_ARGS.map(String::from)).collect();
        // Apart from the quota checks' directory: the two may run at once.
        let cwd = dir.join("catalog");
        std::fs::create_dir_all(&cwd)?;
        let mut spawned = launch::spawn(program, &args, &cwd, &dir.join("gh-empty"))?;
        let io = launch::run(&mut spawned, b"", |stderr| tail(stderr, 4096)).await?;
        let mut stdout = String::new();
        BufReader::new(io.stdout).read_to_string(&mut stdout).await?;
        let status = spawned.child.wait().await?;
        spawned.reap();
        match codex::parse_catalog(&stdout) {
            Some(models) if status.success() => Ok(models),
            _ => anyhow::bail!("{status}: {}", io.stderr.await.unwrap_or_default().trim()),
        }
    }

    /// One quota check: a minimal turn on the harness (data-model.md §8.4). `dir` holds the
    /// directories it runs in.
    pub async fn probe(&self, harness: Harness, permission: Permission, dir: &Path) -> anyhow::Result<CheckOutcome> {
        let name = harness.as_str();
        let (args, input) = match harness {
            Harness::Claude => (claude::probe_args(permission), claude::PROBE_INPUT),
            Harness::Codex => (codex::probe_args(permission), codex::PROBE_INPUT),
        };
        let (program, prefix) = self.cli(harness).program()?;
        let args: Vec<String> = prefix.iter().cloned().chain(args).collect();
        let cwd = dir.join("probe");
        std::fs::create_dir_all(&cwd)?;
        let mut spawned = launch::spawn(program, &args, &cwd, &dir.join("gh-empty"))?;
        let io = launch::run(&mut spawned, input.as_bytes(), |stderr| tail(stderr, 4096)).await?;
        let stderr_tail = io.stderr;
        let read = async {
            let mut lines = BufReader::new(io.stdout).lines();
            let mut output = Output::new(harness);
            let (mut completed, mut resets_at) = (false, None);
            while let Some(line) = lines.next_line().await? {
                for event in output.parse_line(&line) {
                    match event {
                        Event::TurnCompleted { .. } => completed = true,
                        Event::QuotaRejected { resets_at: Some(at), .. } => resets_at = Some(at),
                        _ => {}
                    }
                }
            }
            Ok::<_, std::io::Error>((completed, resets_at))
        };
        let (completed, resets_at) = match tokio::time::timeout(self.probe_timeout, read).await {
            Ok(read) => read?,
            Err(_) => {
                tracing::warn!(harness = name, timeout = ?self.probe_timeout, "quota check timed out");
                (false, None)
            }
        };
        let _ = spawned.child.start_kill();
        let _ = spawned.child.wait().await;
        spawned.reap();
        if !completed && let Ok(Ok(tail)) = tokio::time::timeout(Duration::from_secs(5), stderr_tail).await {
            tracing::warn!(harness = name, stderr = tail, "quota check did not complete");
        }
        // Claude says when the quota resets; Codex's rejection format is not known yet, so its
        // failed check carries no reset time (data-model.md §8.3).
        Ok(if completed { CheckOutcome::Passed } else { CheckOutcome::Rejected { resets_at } })
    }
}

/// Reads a stream to its end and keeps the last `keep` bytes.
async fn tail(mut stream: impl AsyncRead + Unpin, keep: usize) -> String {
    let (mut kept, mut buf) = (Vec::new(), vec![0u8; 8192]);
    while let Ok(n) = stream.read(&mut buf).await {
        if n == 0 {
            break;
        }
        kept.extend_from_slice(&buf[..n]);
        if kept.len() > keep {
            kept.drain(..kept.len() - keep);
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}
