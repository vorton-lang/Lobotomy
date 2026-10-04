//! The host: what all projects of this user share, outside any project (data-model.md §10).
//! v1 keeps the quota domains and the harness configuration here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use lobotomy_core::id::now_ms;
use lobotomy_core::quota::{CheckOutcome, Domain, HostDb};
use lobotomy_harness::codex;
use lobotomy_harness::event::Event;
use lobotomy_harness::process::{self, Spec};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct Host {
    pub dir: PathBuf,
    pub db: HostDb,
    pub harness: HarnessConfig,
    /// Domains with a check in progress: at most one check per domain (data-model.md §8.4).
    checking: Mutex<HashSet<String>>,
}

/// How to start the harness CLIs. Binary detection is host state (data-model.md §10).
#[derive(Clone, Debug)]
pub struct HarnessConfig {
    /// The Codex program and any arguments that go before the adapter's own. Tests put a fake
    /// CLI here.
    pub codex: Vec<String>,
    /// The program and arguments that send Ctrl+C to a pid (harness-adapter.md §1.8).
    pub interrupt_helper: Vec<String>,
    pub codex_reasoning_effort: Option<String>,
}

impl HarnessConfig {
    /// The real CLIs, with this executable as the interrupt helper.
    pub fn detect() -> anyhow::Result<Self> {
        let exe = std::env::current_exe().context("locating the backend executable")?;
        Ok(Self {
            codex: vec![codex::locate().to_string_lossy().into_owned()],
            interrupt_helper: vec![exe.to_string_lossy().into_owned(), "ctrl-c".into()],
            codex_reasoning_effort: None,
        })
    }
}

impl Host {
    pub fn open(dir: &Path, harness: HarnessConfig) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let db = HostDb::open(&dir.join("host.db")).context("opening the host database")?;
        Ok(Self { dir: dir.to_path_buf(), db, harness, checking: Mutex::new(HashSet::new()) })
    }

    /// `%LOCALAPPDATA%\Lobotomy` on Windows; `$XDG_STATE_HOME/lobotomy` or
    /// `~/.local/state/lobotomy` elsewhere.
    pub fn default_dir() -> anyhow::Result<PathBuf> {
        if cfg!(windows) {
            let base = std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?;
            return Ok(PathBuf::from(base).join("Lobotomy"));
        }
        if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
            return Ok(PathBuf::from(state).join("lobotomy"));
        }
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        Ok(PathBuf::from(home).join(".local").join("state").join("lobotomy"))
    }

    pub fn is_blocked(&self, harness: &str) -> anyhow::Result<bool> {
        Ok(self.db.domain(harness)?.is_blocked())
    }

    /// The user's retry: cancels the planned check and checks now (data-model.md §8.4).
    pub async fn retry(self: &Arc<Self>, harness: &str) -> anyhow::Result<Domain> {
        self.db.cancel_planned_check(harness)?;
        self.check(harness).await
    }

    /// Runs one quota check for the domain and records the result. A check already in progress
    /// is not repeated; the current state is returned instead.
    pub async fn check(self: &Arc<Self>, harness: &str) -> anyhow::Result<Domain> {
        if !self.checking.lock().unwrap().insert(harness.to_owned()) {
            return Ok(self.db.domain(harness)?);
        }
        let outcome = self.probe(harness).await;
        self.checking.lock().unwrap().remove(harness);
        let outcome = outcome.unwrap_or_else(|e| {
            tracing::warn!(harness, error = format!("{e:#}"), "quota check could not run");
            CheckOutcome::Rejected { resets_at: None }
        });
        tracing::info!(harness, ?outcome, "quota check");
        Ok(self.db.record_check(harness, &outcome, now_ms())?)
    }

    async fn probe(&self, harness: &str) -> anyhow::Result<CheckOutcome> {
        anyhow::ensure!(harness == "codex", "no quota check for {harness} yet (Claude arrives with M3)");
        let (program, prefix) = self.harness.codex.split_first().context("no Codex program configured")?;
        let args: Vec<String> = prefix.iter().cloned().chain(codex::probe_args()).collect();
        let cwd = self.dir.join("probe");
        std::fs::create_dir_all(&cwd)?;
        let mut spawned = process::spawn(&Spec { program: Path::new(program), args: &args, cwd: &cwd, env: &[] })?;
        spawned.resume()?;
        let mut stdin = spawned.child.stdin.take().context("no stdin")?;
        let _ = stdin.write_all(codex::PROBE_INPUT.as_bytes()).await;
        drop(stdin);
        let stdout = spawned.child.stdout.take().context("no stdout")?;
        let mut lines = BufReader::new(stdout).lines();
        let mut completed = false;
        while let Some(line) = lines.next_line().await? {
            completed |= matches!(codex::parse_line(&line), Some(Event::TurnCompleted { .. }));
        }
        let _ = spawned.child.wait().await;
        spawned.reap();
        // Codex's rejection format is not known yet, so a failed check carries no reset time and
        // the domain waits for the user (data-model.md §8.3, §8.4).
        Ok(if completed { CheckOutcome::Passed } else { CheckOutcome::Rejected { resets_at: None } })
    }
}
