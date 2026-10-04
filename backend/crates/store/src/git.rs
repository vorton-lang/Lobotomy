//! Runs the git CLI. The store uses it where git's own behavior is what we want: fetching between
//! repositories, refs, and the git view of a materialized directory.

use std::path::Path;
use std::process::{Command, Output};

use crate::error::{Error, Result};

pub(crate) struct Git<'a> {
    /// `git -C <dir>`.
    pub dir: Option<&'a Path>,
    /// `git --git-dir <dir>`.
    pub git_dir: Option<&'a Path>,
}

impl<'a> Git<'a> {
    pub fn at(dir: &'a Path) -> Self {
        Git { dir: Some(dir), git_dir: None }
    }

    pub fn bare(git_dir: &'a Path) -> Self {
        Git { dir: None, git_dir: Some(git_dir) }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        if let Some(dir) = self.dir {
            cmd.arg("-C").arg(dir);
        }
        if let Some(git_dir) = self.git_dir {
            cmd.arg("--git-dir").arg(git_dir);
        }
        cmd.args(args).env("GIT_TERMINAL_PROMPT", "0");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    }

    /// Runs git and returns its exit status and output, whatever the status.
    pub fn output(&self, args: &[&str]) -> Result<Output> {
        self.command(args).output().map_err(|e| Error::Git { args: args.join(" "), message: e.to_string() })
    }

    /// Runs git and returns stdout, trimmed. A non-zero exit is an error carrying stderr.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let out = self.output(args)?;
        if !out.status.success() {
            return Err(Error::Git {
                args: args.join(" "),
                message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// Runs git with `input` on stdin and extra environment variables; returns raw stdout.
    pub fn run_with_input(&self, args: &[&str], input: &[u8], env: &[(&str, &str)]) -> Result<Vec<u8>> {
        use std::io::Write as _;
        use std::process::Stdio;
        let error = |message: String| Error::Git { args: args.join(" "), message };
        let mut cmd = self.command(args);
        cmd.envs(env.iter().copied()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| error(e.to_string()))?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // Writing on another thread: git may fill stdout before it has read all of stdin.
        let input = input.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&input));
        let out = child.wait_with_output().map_err(|e| error(e.to_string()))?;
        writer.join().expect("the stdin writer does not panic").map_err(|e| error(e.to_string()))?;
        if !out.status.success() {
            return Err(error(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
        }
        Ok(out.stdout)
    }

    /// Runs git where exit code 1 means "no such thing", as for `config` or `symbolic-ref -q`.
    pub fn optional(&self, args: &[&str]) -> Result<Option<String>> {
        let out = self.output(args)?;
        match out.status.code() {
            Some(0) => Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())),
            Some(1) => Ok(None),
            _ => Err(Error::Git { args: args.join(" "), message: String::from_utf8_lossy(&out.stderr).trim().to_owned() }),
        }
    }
}
