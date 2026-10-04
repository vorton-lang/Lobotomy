//! The host: what all projects of this user share, outside any project (data-model.md §10).
//! v1 keeps the quota domains and the harness configuration here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use lobotomy_core::quota::{CheckOutcome, Domain, HostDb};
use lobotomy_harness::codex;
use lobotomy_harness::event::Event;
use lobotomy_harness::process::{self, Spec};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::capability;

pub struct Host {
    pub dir: PathBuf,
    pub db: HostDb,
    pub harness: HarnessConfig,
    /// What a GUI connection must present (frontend.md §1). Generated once, kept in
    /// `<dir>/gui-token`; never put into the environment of roles or checks.
    pub gui_token: String,
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
    /// A quota check that has not finished by then counts as rejected without a reset time; the
    /// domain stays blocked (data-model.md §8.4). A normal check takes seconds.
    pub probe_timeout: Duration,
}

impl HarnessConfig {
    /// The real CLIs, with this executable as the interrupt helper.
    /// `LOBOTOMY_CODEX`, a JSON array of program and arguments, replaces the located Codex. The
    /// GUI's end-to-end tests put the fake CLI there.
    pub fn detect() -> anyhow::Result<Self> {
        let exe = std::env::current_exe().context("locating the backend executable")?;
        let codex = match std::env::var("LOBOTOMY_CODEX") {
            Ok(json) => serde_json::from_str(&json).context("LOBOTOMY_CODEX is not a JSON array of strings")?,
            Err(_) => vec![codex::locate().to_string_lossy().into_owned()],
        };
        Ok(Self {
            codex,
            interrupt_helper: vec![exe.to_string_lossy().into_owned(), "ctrl-c".into()],
            codex_reasoning_effort: None,
            probe_timeout: Duration::from_secs(120),
        })
    }
}

/// Marks a domain as being checked, and clears the mark however the check ends: done, failed,
/// timed out or cancelled.
struct CheckingMark<'a> {
    checking: &'a Mutex<HashSet<String>>,
    harness: String,
}

impl<'a> CheckingMark<'a> {
    fn acquire(checking: &'a Mutex<HashSet<String>>, harness: &str) -> Option<Self> {
        checking.lock().unwrap().insert(harness.to_owned()).then(|| Self { checking, harness: harness.to_owned() })
    }
}

impl Drop for CheckingMark<'_> {
    fn drop(&mut self) {
        self.checking.lock().unwrap().remove(&self.harness);
    }
}

impl Host {
    pub fn open(dir: &Path, harness: HarnessConfig) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let db = HostDb::open(&dir.join("host.db")).context("opening the host database")?;
        let gui_token = gui_token(&dir.join("gui-token"))?;
        Ok(Self { dir: dir.to_path_buf(), db, harness, gui_token, checking: Mutex::new(HashSet::new()) })
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

    /// The user's retry: one quota check for the domain, with the result recorded
    /// (data-model.md §8.4). The only way a blocked domain opens again. A check already in
    /// progress is not repeated; the current state is returned instead.
    pub async fn retry(self: &Arc<Self>, harness: &str) -> anyhow::Result<Domain> {
        let Some(_mark) = CheckingMark::acquire(&self.checking, harness) else {
            return Ok(self.db.domain(harness)?);
        };
        let outcome = self.probe(harness).await.unwrap_or_else(|e| {
            tracing::warn!(harness, error = format!("{e:#}"), "quota check could not run");
            CheckOutcome::Rejected { resets_at: None }
        });
        tracing::info!(harness, ?outcome, "quota check");
        Ok(self.db.record_check(harness, &outcome)?)
    }

    async fn probe(&self, harness: &str) -> anyhow::Result<CheckOutcome> {
        anyhow::ensure!(harness == "codex", "no quota check for {harness} yet (Claude arrives with M3)");
        let (program, prefix) = self.harness.codex.split_first().context("no Codex program configured")?;
        let args: Vec<String> = prefix.iter().cloned().chain(codex::probe_args()).collect();
        let cwd = self.dir.join("probe");
        let gh = self.dir.join("gh-empty");
        for dir in [&cwd, &gh] {
            std::fs::create_dir_all(dir)?;
        }
        let env = capability::env(&gh);
        let spec = Spec { program: Path::new(program), args: &args, cwd: &cwd, env: &env, env_remove: capability::REMOVED_VARS };
        let mut spawned = process::spawn(&spec)?;
        spawned.resume()?;
        let mut stdin = spawned.child.stdin.take().context("no stdin")?;
        let _ = stdin.write_all(codex::PROBE_INPUT.as_bytes()).await;
        drop(stdin);
        // Both pipes are read, so the CLI never blocks on a full pipe (#10).
        let stderr = spawned.child.stderr.take().context("no stderr")?;
        let stderr_tail = tokio::spawn(tail(stderr, 4096));
        let stdout = spawned.child.stdout.take().context("no stdout")?;
        let read = async {
            let mut lines = BufReader::new(stdout).lines();
            let mut completed = false;
            while let Some(line) = lines.next_line().await? {
                completed |= matches!(codex::parse_line(&line), Some(Event::TurnCompleted { .. }));
            }
            Ok::<_, std::io::Error>(completed)
        };
        let completed = match tokio::time::timeout(self.harness.probe_timeout, read).await {
            Ok(completed) => completed?,
            Err(_) => {
                tracing::warn!(harness, timeout = ?self.harness.probe_timeout, "quota check timed out");
                false
            }
        };
        let _ = spawned.child.start_kill();
        let _ = spawned.child.wait().await;
        spawned.reap();
        if !completed && let Ok(Ok(tail)) = tokio::time::timeout(Duration::from_secs(5), stderr_tail).await {
            tracing::warn!(harness, stderr = tail, "quota check did not complete");
        }
        // Codex's rejection format is not known yet, so a failed check carries no reset time
        // (data-model.md §8.3).
        Ok(if completed { CheckOutcome::Passed } else { CheckOutcome::Rejected { resets_at: None } })
    }
}

/// The GUI token, created on first use: two ULIDs give 160 random bits.
fn gui_token(path: &Path) -> anyhow::Result<String> {
    if let Ok(token) = std::fs::read_to_string(path)
        && !token.trim().is_empty()
    {
        return Ok(token.trim().to_owned());
    }
    let token = format!("{}{}", ulid_part(), ulid_part());
    std::fs::write(path, &token).with_context(|| format!("writing {}", path.display()))?;
    Ok(token)
}

fn ulid_part() -> String {
    let id = lobotomy_core::id::new_id("t");
    id.trim_start_matches("t_").to_lowercase()
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
