//! The host: what all projects of this user share, outside any project (data-model.md §10):
//! the project registry, the quota domains, the harness configuration, the listening address and
//! the GUI connections.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use lobotomy_core::Error;
use lobotomy_core::host::HostDb;
use lobotomy_core::quota::{CheckOutcome, Domain};
use lobotomy_harness::event::Event;
use lobotomy_harness::{Harness, Output, Permission, claude, codex};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::sync::{Notify, broadcast};

use crate::launch;

pub struct Host {
    pub dir: PathBuf,
    pub db: HostDb,
    pub harness: HarnessConfig,
    /// What a GUI connection must present (frontend.md §1). Generated once, kept in
    /// `<dir>/gui-token`; never put into the environment of roles or checks.
    pub gui_token: String,
    /// The MCP service's base URL, such as `http://127.0.0.1:4100`. Set once the server listens.
    pub mcp_base: Mutex<String>,
    /// What connected GUIs are told (see `gui`), as JSON text. Pushes about a project name it.
    pub gui_push: broadcast::Sender<Arc<str>>,
    /// The GUI asked the backend to stop (frontend.md §1).
    pub shutdown_requested: Notify,
    /// Domains with a check in progress: at most one check per domain (data-model.md §8.4).
    checking: Mutex<HashSet<String>>,
}

/// How to start the harness CLIs. Binary detection is host state (data-model.md §10).
#[derive(Clone, Debug)]
pub struct HarnessConfig {
    pub claude: Cli,
    pub codex: Cli,
    /// The program and arguments that send Ctrl+C to a pid (harness-adapter.md §1.8).
    pub interrupt_helper: Vec<String>,
    /// The backend executable, which runs a trial's terminal as `trial-terminal` (frontend.md §8).
    pub terminal_helper: PathBuf,
    /// A quota check that has not finished by then counts as rejected without a reset time; the
    /// domain stays blocked (data-model.md §8.4). A normal check takes seconds.
    pub probe_timeout: Duration,
}

/// How to start one harness's CLI.
#[derive(Clone, Debug)]
pub struct Cli {
    /// The program and any arguments that go before the adapter's own. Tests put a fake CLI here.
    pub command: Vec<String>,
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
    /// The real CLIs, with this executable as the interrupt and terminal helper.
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
            terminal_helper: exe,
            probe_timeout: Duration::from_secs(120),
        })
    }

    pub fn cli(&self, harness: Harness) -> &Cli {
        match harness {
            Harness::Claude => &self.claude,
            Harness::Codex => &self.codex,
        }
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
        Ok(Self {
            dir: dir.to_path_buf(),
            db,
            harness,
            gui_token,
            mcp_base: Mutex::new(String::new()),
            gui_push: broadcast::channel(crate::gui::PUSH_BUFFER).0,
            shutdown_requested: Notify::new(),
            checking: Mutex::new(HashSet::new()),
        })
    }

    /// Where new projects keep their data, one directory per project id (data-model.md §10.3).
    pub fn projects_dir(&self) -> PathBuf {
        self.dir.join("projects")
    }

    /// Tells the GUIs to take a new host snapshot: a host setting or the project list changed.
    /// Neither has an event of its own.
    pub fn changed(&self) {
        let _ = self.gui_push.send(serde_json::json!({ "type": "host" }).to_string().into());
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

    /// The harness's permission mode (harness-adapter.md §1.9): full access until the user
    /// changes it. Each turn reads it, so a change applies from the next turn on.
    pub fn permission(&self, harness: Harness) -> anyhow::Result<Permission> {
        match self.db.permission(harness.as_str())? {
            None => Ok(Permission::default()),
            Some(value) => Permission::parse(&value).ok_or_else(|| {
                Error::invariant(format!(
                    "unknown permission mode {value:?} for {} in the host database",
                    harness.as_str()
                ))
                .into()
            }),
        }
    }

    /// The user's choice; only the GUI makes it.
    pub fn set_permission(&self, harness: &str, permission: &str) -> anyhow::Result<Permission> {
        let Some(harness) = Harness::parse(harness) else {
            return Err(Error::rejected("unknown_harness", format!("unknown harness {harness}")).into());
        };
        let Some(parsed) = Permission::parse(permission) else {
            return Err(Error::rejected("unknown_permission", format!("unknown permission mode {permission}")).into());
        };
        self.db.set_permission(harness.as_str(), parsed.as_str(), lobotomy_core::id::now_ms())?;
        tracing::info!(harness = harness.as_str(), permission, "permission mode changed");
        Ok(parsed)
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

    async fn probe(&self, name: &str) -> anyhow::Result<CheckOutcome> {
        let harness = Harness::parse(name).with_context(|| format!("no quota check for {name}"))?;
        let permission = self.permission(harness)?;
        let (args, input) = match harness {
            Harness::Claude => (claude::probe_args(permission), claude::PROBE_INPUT),
            Harness::Codex => (codex::probe_args(permission), codex::PROBE_INPUT),
        };
        let (program, prefix) = self.harness.cli(harness).program()?;
        let args: Vec<String> = prefix.iter().cloned().chain(args).collect();
        let cwd = self.dir.join("probe");
        std::fs::create_dir_all(&cwd)?;
        let what = launch::What::Program { program, args: &args };
        let mut spawned = launch::spawn(what, &cwd, &self.dir.join("gh-empty")).await?;
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
        let (completed, resets_at) = match tokio::time::timeout(self.harness.probe_timeout, read).await {
            Ok(read) => read?,
            Err(_) => {
                tracing::warn!(harness = name, timeout = ?self.harness.probe_timeout, "quota check timed out");
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
