//! Directories materialized from the store: slots and the verification site (harness-adapter.md
//! §3). Each is an ordinary git clone whose objects come from the store through alternates, with
//! HEAD at the baseline, so the agent's `git status` and `git diff` show the work. jj's working
//! copy state lives outside the directory, in `state_dir`; the agent cannot touch it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::local_working_copy::{TreeState, TreeStateSettings};
use jj_lib::matchers::{EverythingMatcher, Matcher, NothingMatcher, PrefixMatcher};
use jj_lib::repo_path::{RepoPath, RepoPathBuf, RepoPathComponent};
use jj_lib::working_copy::{SnapshotOptions, UntrackedReason};
use pollster::FutureExt as _;
use serde::{Deserialize, Serialize};

use crate::eol;
use crate::error::{Error, Result};
use crate::git::Git;
use crate::store::{Identity, Store, path_arg};

/// What a capture covers, from the project configuration (harness-adapter.md §4.1). The
/// directory's own ignore files apply as well, as they are at capture time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    /// gitignore patterns that are never captured, such as `target/`. Materializing keeps them
    /// as caches.
    pub excluded: Vec<String>,
    /// Paths (prefixes, relative to the root) captured even when an ignore rule matches them.
    pub force_tracked: Vec<String>,
}

/// The size guardrail: a capture that adds more than this stops before anything is written
/// (harness-adapter.md §4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guard {
    pub max_new_files: usize,
    pub max_new_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewFile {
    pub path: String,
    pub size: u64,
}

/// Content a capture cannot represent. Capturing fails closed on it (harness-adapter.md §4.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum Uncovered {
    /// A directory with its own `.git` or `.jj`. jj skips it entirely, and files it tracks inside
    /// would be recorded as deleted.
    NestedRepo(String),
    /// Neither a file, a directory nor a symlink.
    SpecialFile(String),
    /// A name that is not valid UTF-8, given as the parent directory and the lossy name.
    InvalidName(String),
}

/// What a capture may leave out, by the user's decision on a capture that was stopped
/// (harness-adapter.md §4.1). Left-out content stays on disk until the slot is materialized again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Leave {
    /// Files not tracked yet. Changes to tracked files are still captured.
    pub new_files: bool,
    /// Nested repositories, special files and invalid names.
    pub uncovered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Captured {
    /// The directory's content as a commit on the baseline, pinned.
    Pinned { commit: String },
    /// The guardrail stopped the capture; nothing was written.
    Oversized { files: Vec<NewFile>, total_bytes: u64 },
    /// Content the capture would lose; nothing was written.
    Uncovered { paths: Vec<Uncovered> },
}

pub struct Workspace {
    pub path: PathBuf,
    pub state_dir: PathBuf,
}

struct Matchers {
    ignores: Arc<GitIgnoreFile>,
    force: PrefixMatcher,
}

impl Matchers {
    fn new(scope: &Scope) -> Result<Self> {
        let patterns = scope.excluded.join("\n");
        let ignores = GitIgnoreFile::empty()
            .chain(RepoPath::root(), Path::new("lobotomy-scope"), patterns.as_bytes())
            .map_err(Error::jj)?;
        let force: Vec<RepoPathBuf> = scope
            .force_tracked
            .iter()
            .map(|p| RepoPathBuf::from_relative_path(Path::new(p.trim_end_matches(['/', '\\']))))
            .collect::<std::result::Result<_, _>>()
            .map_err(Error::jj)?;
        Ok(Matchers { ignores, force: PrefixMatcher::new(&force) })
    }
}

impl Workspace {
    pub fn new(path: impl Into<PathBuf>, state_dir: impl Into<PathBuf>) -> Self {
        Workspace { path: path.into(), state_dir: state_dir.into() }
    }

    fn load(&self, store: &Store) -> Result<TreeState> {
        fs::create_dir_all(&self.state_dir).map_err(Error::io(&self.state_dir))?;
        let settings = TreeStateSettings::try_from_user_settings(&store.settings).map_err(Error::jj)?;
        TreeState::load(store.inner.clone(), self.path.clone(), self.state_dir.clone(), &settings).map_err(Error::jj)
    }

    /// Writes `target` into the directory and points git's HEAD at `head` on `branch`
    /// (harness-adapter.md §3). Files the store tracks are brought to `target`; files nobody
    /// captured are removed; ignored files, such as build caches, stay. Creates the directory on
    /// first use.
    ///
    /// The caller makes sure the old content was captured first. On [`Error::Blocked`] the
    /// directory does not equal the target; the caller moves it aside and materializes a fresh
    /// generation. Interrupted git setup and checkouts can be retried in place.
    pub fn materialize(&self, store: &Store, target: &str, head: &str, branch: &str, scope: &Scope) -> Result<()> {
        let git = Git::at(&self.path);
        if !self.path.join(".git").exists() {
            fs::create_dir_all(&self.path).map_err(Error::io(&self.path))?;
            git.run(&["init", "--quiet", "--initial-branch", branch])?;
        }
        // Repeat setup: .git may exist after an interruption before these writes finished.
        // Line endings follow the repository (see `eol`). git in the slot must apply the
        // repository's `.gitattributes` and nothing from the machine, or the agent's `git
        // diff` and `git checkout` would disagree with what is captured.
        git.run(&["config", "core.autocrlf", "false"])?;
        git.run(&["config", "core.eol", "lf"])?;
        let no_attributes = self.path.join(".git").join("info").join("no-global-attributes");
        git.run(&["config", "core.attributesFile", &path_arg(&no_attributes)])?;
        let alternates = self.path.join(".git").join("objects").join("info").join("alternates");
        let objects = store.git_dir().join("objects");
        fs::write(&alternates, format!("{}\n", path_arg(&objects))).map_err(Error::io(&alternates))?;
        let matchers = Matchers::new(scope)?;
        let mut tree_state = self.load(store)?;
        // Learn what changed on disk, without tracking anything new, so that checking out
        // restores modified and missing files and drops files that nobody captured. This also
        // finishes a checkout that was interrupted, and rebuilds a directory that was moved away.
        let options = SnapshotOptions {
            base_ignores: matchers.ignores.clone(),
            progress: None,
            start_tracking_matcher: &NothingMatcher,
            force_tracking_matcher: &NothingMatcher,
            max_new_file_size: u64::MAX,
        };
        let (_, stats) = tree_state.snapshot(&options).block_on().map_err(Error::jj)?;
        for path in stats.untracked_paths.keys() {
            let disk = path.to_fs_path(&self.path).map_err(Error::jj)?;
            fs::remove_file(&disk).map_err(Error::io(&disk))?;
        }
        let target = store.commit(target)?;
        let before = tree_state.current_tree().clone();
        let checkout = tree_state.check_out(&target.tree()).map_err(Error::jj)?;
        tree_state.save().map_err(Error::jj)?;
        if checkout.skipped_files > 0 {
            return Err(Error::Blocked(checkout.skipped_files));
        }
        eol::apply_to_worktree(store, &self.path, &before, &target.tree())?;

        let reference = format!("refs/heads/{branch}");
        git.run(&["symbolic-ref", "HEAD", &reference])?;
        git.run(&["update-ref", &reference, head])?;
        git.run(&["read-tree", head])?;
        // Fills in the index's stat data so the first `git status` is fast. It reports files that
        // differ from HEAD with a non-zero exit, which is expected.
        let _ = git.output(&["update-index", "-q", "--refresh"]);
        Ok(())
    }

    /// Captures the directory as a commit on `base` and pins it under `pin` (harness-adapter.md
    /// §4.1). The writer must have stopped. With an existing pin, returns the pinned commit
    /// without looking at the directory again (#6 §2).
    ///
    /// `leave` captures what can be captured and leaves the rest out; it is the user's decision,
    /// never a default.
    pub fn capture(
        &self,
        store: &Store,
        base: &str,
        scope: &Scope,
        guard: Option<&Guard>,
        leave: Leave,
        pin: &str,
    ) -> Result<Captured> {
        if let Some(commit) = store.pinned(pin)? {
            return Ok(Captured::Pinned { commit });
        }
        let matchers = Matchers::new(scope)?;
        let mut tree_state = self.load(store)?;

        let mut scan = Scan::default();
        scan.walk(RepoPath::root(), &self.path, matchers.ignores.clone(), &matchers, &tree_state)?;
        if !scan.uncovered.is_empty() && !leave.uncovered {
            return Ok(Captured::Uncovered { paths: scan.uncovered });
        }
        if let Some(guard) = guard
            && !leave.new_files
        {
            let total_bytes: u64 = scan.new_files.iter().map(|f| f.size).sum();
            if scan.new_files.len() > guard.max_new_files || total_bytes > guard.max_new_bytes {
                return Ok(Captured::Oversized { files: scan.new_files, total_bytes });
            }
        }

        // Leaving new files out means tracking nothing new: changes to tracked files are kept.
        let start_tracking: &dyn Matcher = if leave.new_files { &NothingMatcher } else { &EverythingMatcher };
        let options = SnapshotOptions {
            base_ignores: matchers.ignores.clone(),
            progress: None,
            start_tracking_matcher: start_tracking,
            force_tracking_matcher: &matchers.force,
            max_new_file_size: u64::MAX,
        };
        let before = tree_state.current_tree().clone();
        let (_, stats) = tree_state.snapshot(&options).block_on().map_err(Error::jj)?;
        if !leave.new_files
            && let Some((path, reason)) = stats.untracked_paths.iter().next()
        {
            // Everything matches the start-tracking matcher and there is no size limit, so jj
            // leaves nothing new untracked.
            let reason = match reason {
                UntrackedReason::FileTooLarge { size, .. } => format!("too large ({size} bytes)"),
                UntrackedReason::FileNotAutoTracked => "not auto-tracked".to_owned(),
            };
            return Err(Error::Invalid(format!("{} stayed untracked: {reason}", path.as_internal_file_string())));
        }
        if !stats.invalid_utf8_paths.is_empty() && !leave.uncovered {
            let paths = stats
                .invalid_utf8_paths
                .iter()
                .map(|(dir, name)| Uncovered::InvalidName(join_lossy(dir, &name.to_string_lossy())))
                .collect();
            return Ok(Captured::Uncovered { paths });
        }
        let parent = store.commit(base)?;
        let after = tree_state.current_tree().clone();
        if let Some(normalized) = eol::normalize_capture(store, &self.path, &before, &after, &parent.tree())? {
            // The rewritten files are read again at the next snapshot.
            tree_state.reset(&normalized).block_on().map_err(Error::jj)?;
        }
        let commit = store.write_commit(
            &parent,
            tree_state.current_tree().clone(),
            &format!("capture {pin}"),
            &Identity::runtime(),
        )?;
        store.pin(pin, &commit)?;
        tree_state.save().map_err(Error::jj)?;
        Ok(Captured::Pinned { commit })
    }
}

/// The walk before a capture: finds new files for the guardrail and content the capture cannot
/// represent. It follows the rules jj's snapshot applies, so both agree on what is new.
#[derive(Default)]
struct Scan {
    new_files: Vec<NewFile>,
    uncovered: Vec<Uncovered>,
}

impl Scan {
    fn walk(
        &mut self,
        dir: &RepoPath,
        disk_dir: &Path,
        ignores: Arc<GitIgnoreFile>,
        matchers: &Matchers,
        tree_state: &TreeState,
    ) -> Result<()> {
        let ignores = ignores.chain_with_file(dir, disk_dir.join(".gitignore")).map_err(Error::jj)?;
        let entries = fs::read_dir(disk_dir).map_err(Error::io(disk_dir))?;
        for entry in entries {
            let entry = entry.map_err(Error::io(disk_dir))?;
            let name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(name) => {
                    self.uncovered.push(Uncovered::InvalidName(join_lossy(dir, &name.to_string_lossy())));
                    continue;
                }
            };
            // The directory's own repository, or a marker that its parent's check handles.
            if name == ".git" || name == ".jj" {
                continue;
            }
            let component = RepoPathComponent::new(&name).map_err(Error::jj)?;
            let path = dir.join(component);
            let file_type = entry.file_type().map_err(Error::io(entry.path()))?;
            let states = tree_state.file_states();
            if file_type.is_dir() {
                let tracked_inside = !states.prefixed(&path).is_empty();
                let ignored = ignores.matches_dir(&path) && matchers.force.visit(&path).is_nothing();
                let nested = [".git", ".jj"].iter().any(|n| entry.path().join(n).symlink_metadata().is_ok());
                if nested {
                    // An ignored repository nobody tracks into, such as one under
                    // `node_modules/`, would not be captured anyway.
                    if !ignored || tracked_inside {
                        self.uncovered.push(Uncovered::NestedRepo(path.as_internal_file_string().to_owned()));
                    }
                    continue;
                }
                if ignored {
                    // jj looks only at tracked files here; none of them is new.
                    continue;
                }
                self.walk(&path, &entry.path(), ignores.clone(), matchers, tree_state)?;
            } else {
                let tracked = states.contains_path(&path);
                let ignored = ignores.matches_file(&path) && !matchers.force.matches(&path);
                if file_type.is_file() || file_type.is_symlink() {
                    if !tracked && !ignored {
                        let size = entry.metadata().map_err(Error::io(entry.path()))?.len();
                        self.new_files.push(NewFile { path: path.as_internal_file_string().to_owned(), size });
                    }
                } else if tracked || !ignored {
                    self.uncovered.push(Uncovered::SpecialFile(path.as_internal_file_string().to_owned()));
                }
            }
        }
        Ok(())
    }
}

fn join_lossy(dir: &RepoPath, name: &str) -> String {
    if dir.is_root() { name.to_owned() } else { format!("{}/{name}", dir.as_internal_file_string()) }
}
