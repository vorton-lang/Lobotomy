//! The user's repository: checks for onboarding and the read-only preview of the integration
//! version (harness-adapter.md §4.3). Lobotomy only ever fast-forwards the recorded branch, and
//! only when the repository is exactly as the last preview left it.

use std::path::Path;

use crate::error::{Error, Result};
use crate::git::Git;
use crate::store::{Identity, Store, path_arg};

/// The repository as git sees it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoState {
    /// The checked-out branch; `None` for a detached HEAD.
    pub branch: Option<String>,
    /// `None` before the first commit.
    pub head: Option<String>,
    /// Changed tracked files and untracked files that are not ignored, as `git status` lists them.
    pub dirty: Vec<String>,
}

pub fn inspect(repo: &Path) -> Result<RepoState> {
    let git = Git::at(repo);
    git.run(&["rev-parse", "--git-dir"])?;
    let branch = git.optional(&["symbolic-ref", "-q", "--short", "HEAD"])?;
    let head = git.optional(&["rev-parse", "-q", "--verify", "HEAD^{commit}"])?;
    let args = ["status", "--porcelain=v1", "-z", "--untracked-files=all"];
    let out = git.output(&args)?;
    if !out.status.success() {
        return Err(Error::Git {
            args: args.join(" "),
            message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    // Entries are `XY path`, NUL-terminated; a rename or copy is followed by its source path.
    let status = String::from_utf8_lossy(&out.stdout);
    let mut dirty = Vec::new();
    let mut entries = status.split('\0').filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        let (code, path) = entry.split_at(entry.len().min(3));
        dirty.push(path.to_owned());
        if code.starts_with(['R', 'C']) {
            entries.next();
        }
    }
    Ok(RepoState { branch, head, dirty })
}

/// The user's git identity in this repository: integration versions are authored by the user
/// (harness-adapter.md §4.3).
pub fn identity(repo: &Path) -> Result<Option<Identity>> {
    let git = Git::at(repo);
    let name = git.optional(&["config", "user.name"])?;
    let email = git.optional(&["config", "user.email"])?;
    Ok(match (name, email) {
        (Some(name), Some(email)) if !name.is_empty() && !email.is_empty() => Some(Identity { name, email }),
        _ => None,
    })
}

/// Why the preview cannot be written. The user restores the repository and retries
/// (harness-adapter.md §4.3; data-model.md §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Diverged {
    Branch { expected: String, actual: Option<String> },
    Head { expected: String, actual: Option<String> },
    Dirty(Vec<String>),
}

impl std::fmt::Display for Diverged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Diverged::Branch { expected, actual } => match actual {
                Some(actual) => write!(f, "检出的分支是 {actual}，不是 {expected}"),
                None => write!(f, "HEAD 处于 detached 状态，不在 {expected} 上"),
            },
            Diverged::Head { expected, actual } => {
                write!(f, "{} 不在上次预览的提交 {expected} 上", actual.as_deref().unwrap_or("分支"))
            }
            Diverged::Dirty(paths) => write!(f, "工作区有改动：{}", paths.join("、")),
        }
    }
}

/// Checks that the repository is as the last preview left it.
pub fn check_unchanged(repo: &Path, branch: &str, previewed: &str) -> Result<Option<Diverged>> {
    let state = inspect(repo)?;
    if state.branch.as_deref() != Some(branch) {
        return Ok(Some(Diverged::Branch { expected: branch.to_owned(), actual: state.branch }));
    }
    if state.head.as_deref() != Some(previewed) {
        return Ok(Some(Diverged::Head { expected: previewed.to_owned(), actual: state.head }));
    }
    if !state.dirty.is_empty() {
        return Ok(Some(Diverged::Dirty(state.dirty)));
    }
    Ok(None)
}

/// Fast-forwards the checked-out `branch` from `previewed` to `target` and updates the working
/// tree, like `git merge --ff-only` (harness-adapter.md §4.3). Writes nothing when the repository
/// is not as the last preview left it. A repository already at `target` counts as written: the
/// receipt of an earlier run was lost (data-model.md §5).
pub fn fast_forward(
    repo: &Path,
    store: &Store,
    branch: &str,
    previewed: &str,
    target: &str,
) -> Result<Option<Diverged>> {
    if check_unchanged(repo, branch, target)?.is_none() {
        return Ok(None);
    }
    if let Some(diverged) = check_unchanged(repo, branch, previewed)? {
        return Ok(Some(diverged));
    }
    store.export("refs/lobotomy/integration", target)?;
    let git = Git::at(repo);
    git.run(&["fetch", "--no-tags", "--quiet", &path_arg(store.git_dir()), "refs/lobotomy/integration"])?;
    git.run(&["merge", "--ff-only", "--quiet", target])?;
    let head = git.run(&["rev-parse", "HEAD"])?;
    if head != target {
        return Err(Error::Invalid(format!("after the fast-forward HEAD is {head}, not {target}")));
    }
    Ok(None)
}
