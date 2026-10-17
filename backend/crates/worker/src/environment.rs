//! The local-folder environment with a repository (worker-interface.md §1, §7; tech-plan.md §2.3).
//! A bare mirror of the upstream repository; a clone of the mirror for each attempt, on a branch
//! of its own; and what keeps attempts apart (L1): their own temporary directory, git
//! configuration and port range. The agent holds no forge credentials: its clone's origin is the
//! mirror, and the environment takes its commits back once its processes have exited.
//!
//! Every git command the environment runs on a clone gets the agent's git configuration, so both
//! see the same line endings and identity. The clone's hooks never run for the environment.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use lobotomy_worker_protocol::event::ChangeSize;

/// Where the upstream repository is and how the agent's commits are signed.
#[derive(Clone, Debug)]
pub struct Repo {
    /// What `git clone` takes: a URL or a path.
    pub upstream: String,
    /// The branch attempts start from.
    pub base: String,
    pub author_name: String,
    pub author_email: String,
    /// The first attempt's port range starts here; each attempt gets ten ports.
    pub port_start: u16,
}

/// The ports an attempt gets, from `LOBOTOMY_PORT_BASE` on.
pub const PORTS_PER_ATTEMPT: u16 = 10;

/// One attempt's place in the environment.
pub struct AttemptDirs {
    /// The clone the agent works in.
    pub clone: PathBuf,
    pub tmp: PathBuf,
    /// The git configuration the agent and the environment use on the clone instead of the
    /// user's global one.
    pub gitconfig: PathBuf,
}

impl AttemptDirs {
    pub fn new(task_dir: &Path, seq: u32) -> Self {
        Self {
            clone: task_dir.join(format!("a{seq}")),
            tmp: task_dir.join(format!("a{seq}.tmp")),
            gitconfig: task_dir.join("gitconfig"),
        }
    }
}

/// What a delivery took back from the clone.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivered {
    pub head: String,
    pub size: ChangeSize,
    /// The agent left changes uncommitted; the environment committed them.
    pub committed_leftovers: bool,
}

pub struct Environment {
    pub repo: Repo,
    root: PathBuf,
}

impl Environment {
    pub fn new(root: &Path, repo: Repo) -> Self {
        Self { repo, root: root.to_path_buf() }
    }

    pub fn mirror(&self) -> PathBuf {
        self.root.join("mirror.git")
    }

    fn no_hooks(&self) -> PathBuf {
        self.root.join("no-hooks")
    }

    /// The branch an attempt works on, and the mirror's ref its commits are kept under.
    pub fn branch(task_id: &str, seq: u32) -> String {
        let safe: String =
            task_id.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' }).collect();
        format!("lobotomy/{safe}-{seq}")
    }

    /// Clones the upstream repository bare, once, and brings its branches up to date.
    pub fn refresh_mirror(&self) -> anyhow::Result<()> {
        let mirror = self.mirror();
        if !mirror.join("HEAD").exists() {
            let parent = mirror.parent().context("the mirror has no parent directory")?;
            git(parent, &["clone", "--bare", "--quiet", &self.repo.upstream, "mirror.git"], &[])?;
            git(&mirror, &["config", "remote.origin.fetch", "+refs/heads/*:refs/heads/*"], &[])?;
        }
        // Only the upstream's branches are pruned; the attempts' commits are kept elsewhere.
        git(&mirror, &["fetch", "--quiet", "--prune", "origin"], &[])?;
        Ok(())
    }

    /// The variables the agent's processes get (tech-plan.md §2.3 L1): their own temporary
    /// directory, the attempt's git configuration without the system's, and a port range.
    pub fn agent_env(&self, dirs: &AttemptDirs, port_base: u16) -> Vec<(String, String)> {
        let mut env = git_env(dirs);
        let tmp = dirs.tmp.to_string_lossy().into_owned();
        for name in ["TMP", "TEMP", "TMPDIR"] {
            env.push((name.to_owned(), tmp.clone()));
        }
        env.push(("LOBOTOMY_PORT_BASE".to_owned(), port_base.to_string()));
        env
    }

    /// Makes the attempt's clone: from the mirror, on its own branch from the base.
    pub fn prepare(&self, dirs: &AttemptDirs, branch: &str) -> anyhow::Result<()> {
        self.refresh_mirror()?;
        std::fs::create_dir_all(&dirs.tmp).with_context(|| format!("creating {}", dirs.tmp.display()))?;
        std::fs::create_dir_all(self.no_hooks())?;
        std::fs::write(&dirs.gitconfig, self.gitconfig())
            .with_context(|| format!("writing {}", dirs.gitconfig.display()))?;
        if dirs.clone.exists() {
            // A clone a crash interrupted: it starts over.
            std::fs::remove_dir_all(&dirs.clone).with_context(|| format!("removing {}", dirs.clone.display()))?;
        }
        let env = git_env(dirs);
        let parent = dirs.clone.parent().context("the clone has no parent directory")?;
        let mirror = self.mirror().to_string_lossy().into_owned();
        let name = dirs.clone.file_name().context("the clone has no name")?.to_string_lossy().into_owned();
        git(parent, &["clone", "--quiet", "--no-checkout", &mirror, &name], &env)?;
        let start = format!("origin/{}", self.repo.base);
        git(&dirs.clone, &["checkout", "--quiet", "-b", branch, &start], &env)?;
        Ok(())
    }

    /// Takes the attempt's work back once its processes have exited: commits what the agent left
    /// uncommitted, and keeps the commits in the mirror under `keep`. `None` when the branch has
    /// nothing beyond the base.
    pub fn deliver(&self, dirs: &AttemptDirs, keep: &str) -> anyhow::Result<Option<Delivered>> {
        let env = git_env(dirs);
        let clone = &dirs.clone;
        let leftovers = !git(clone, &["status", "--porcelain"], &env)?.is_empty();
        if leftovers {
            git(clone, &["add", "--all"], &env)?;
            let hooks = format!("core.hooksPath={}", self.no_hooks().to_string_lossy());
            let message = "未提交的改动（交付时由环境提交）";
            git(clone, &["-c", &hooks, "commit", "--quiet", "--no-verify", "-m", message], &env)?;
        }
        let head = git(clone, &["rev-parse", "HEAD"], &env)?;
        let base = git(clone, &["merge-base", "HEAD", &format!("refs/remotes/origin/{}", self.repo.base)], &env)?;
        if head == base {
            return Ok(None);
        }
        let source = clone.to_string_lossy().into_owned();
        git(&self.mirror(), &["fetch", "--quiet", "--no-tags", &source, &format!("+HEAD:{keep}")], &env)?;
        let stat = git(clone, &["diff", "--shortstat", &base, &head], &env)?;
        Ok(Some(Delivered { head, size: shortstat(&stat), committed_leftovers: leftovers }))
    }

    fn gitconfig(&self) -> String {
        // No credential helper: the agent has no forge credentials (worker-interface.md §7). Its
        // pushes go nowhere; the environment pushes from the mirror. Line endings stay as the
        // repository has them.
        format!(
            "[credential]\n\thelper =\n[remote \"origin\"]\n\tpushurl = lobotomy-push-disabled://the-environment-pushes\n\
             [core]\n\tautocrlf = false\n\tlongpaths = true\n[user]\n\tname = {}\n\temail = {}\n",
            self.repo.author_name, self.repo.author_email
        )
    }
}

/// The attempt's git configuration in place of the user's and the system's.
fn git_env(dirs: &AttemptDirs) -> Vec<(String, String)> {
    vec![
        ("GIT_CONFIG_GLOBAL".to_owned(), dirs.gitconfig.to_string_lossy().into_owned()),
        ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
    ]
}

/// Runs git in `dir` and returns its trimmed output.
fn git(dir: &Path, args: &[&str], env: &[(String, String)]) -> anyhow::Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args).envs(env.iter().map(|(k, v)| (k, v)));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let out = command.output().with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!("git {} in {}: {}", args.join(" "), dir.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// `git diff --shortstat`'s line, such as "2 files changed, 5 insertions(+), 1 deletion(-)".
fn shortstat(line: &str) -> ChangeSize {
    let mut size = ChangeSize { files: 0, additions: 0, deletions: 0 };
    for part in line.split(',') {
        let mut words = part.split_whitespace();
        let (Some(n), Some(what)) = (words.next().and_then(|n| n.parse().ok()), words.next()) else { continue };
        if what.starts_with("file") {
            size.files = n;
        } else if what.starts_with("insertion") {
            size.additions = n;
        } else if what.starts_with("deletion") {
            size.deletions = n;
        }
    }
    size
}
