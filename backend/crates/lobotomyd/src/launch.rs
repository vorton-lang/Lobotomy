//! Starting the processes the backend runs: role turns, quota checks and project checks. They
//! share what keeps them safe, so no path can leave a part out (#16):
//! - the trimmed environment (harness-adapter.md §1.6);
//! - a job of their own, joined before they run (harness-adapter.md §1.8);
//! - stderr read from the moment they run, so a process that writes much there before it reads
//!   its input cannot block on a full pipe while the backend waits to write that input (#10).
//!
//! Stdout is the caller's to read once the input is written: Codex reads its input before it
//! writes anything on stdout.

use std::future::Future;
use std::path::Path;

use anyhow::Context;
use lobotomy_harness::process::{self, Spawned, Spec};
use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStderr, ChildStdout};
use tokio::task::JoinHandle;

use crate::capability;

/// What to run.
pub enum What<'a> {
    /// A program with its arguments, such as a harness CLI.
    Program { program: &'a Path, args: &'a [String] },
    /// A command line for the platform's shell, such as a project check (harness-adapter.md §4.2).
    Shell(&'a str),
}

/// Starts the process in `cwd`, suspended: it does not run until [`run`]. `gh` is the empty
/// config directory `gh` gets.
pub fn spawn(what: What<'_>, cwd: &Path, gh: &Path) -> anyhow::Result<Spawned> {
    std::fs::create_dir_all(gh).with_context(|| format!("creating {}", gh.display()))?;
    let env = capability::env(gh);
    let (program, args) = match &what {
        What::Program { program, args } => (*program, *args),
        What::Shell(_) => (Path::new(""), &[][..]),
    };
    let spec = Spec { program, args, cwd, env: &env, env_remove: capability::REMOVED_VARS };
    Ok(match what {
        What::Program { .. } => process::spawn(&spec)?,
        What::Shell(command) => process::spawn_shell(command, &spec)?,
    })
}

/// A running process's output, once its input is written.
pub struct Io<E> {
    pub stdout: ChildStdout,
    /// What `read_stderr` made of stderr; it ends when the process and its children close it.
    pub stderr: JoinHandle<E>,
    /// Whether the whole input was written and stdin closed. A process that exits at start, such
    /// as one that refuses its arguments, may close it first.
    pub input: std::io::Result<()>,
}

/// Lets the spawned process run with `read_stderr` already reading its stderr, then writes
/// `input` to its stdin and closes it.
pub async fn run<E, F>(
    spawned: &mut Spawned,
    input: &[u8],
    read_stderr: impl FnOnce(ChildStderr) -> F,
) -> anyhow::Result<Io<E>>
where
    F: Future<Output = E> + Send + 'static,
    E: Send + 'static,
{
    let stderr = spawned.child.stderr.take().context("no stderr")?;
    let stdout = spawned.child.stdout.take().context("no stdout")?;
    let mut stdin = spawned.child.stdin.take().context("no stdin")?;
    spawned.resume()?;
    let stderr = tokio::spawn(read_stderr(stderr));
    let input = async {
        stdin.write_all(input).await?;
        stdin.shutdown().await
    }
    .await;
    drop(stdin);
    Ok(Io { stdout, stderr, input })
}
