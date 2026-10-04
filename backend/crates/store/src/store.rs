//! The private store: a git-backed jj store in the project's data directory (harness-adapter.md
//! §4). Commits are written straight to the store; SQLite, not jj's operation log, records what
//! they mean. A pin is a ref named after the record that owns the commit, so the commit survives
//! whatever jj or git do with unreferenced objects (#6 §2).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jj_lib::backend::{self, CommitId, Signature, Timestamp};
use jj_lib::commit::Commit;
use jj_lib::config::{ConfigLayer, ConfigSource, StackedConfig};
use jj_lib::git_backend::GitBackend;
use jj_lib::merge::Merge;
use jj_lib::merged_tree::MergedTree;
use jj_lib::object_id::ObjectId as _;
use jj_lib::settings::UserSettings;
use jj_lib::signing::Signer;
use jj_lib::tree_merge::MergeOptions;
use pollster::FutureExt as _;

use crate::error::{Error, Result};
use crate::git::Git;

const PIN_NAMESPACE: &str = "refs/lobotomy/pins/";

/// Who a commit is attributed to.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

impl Identity {
    /// The runtime itself, for internal commits such as captures.
    pub fn runtime() -> Self {
        Identity { name: "Lobotomy".into(), email: "lobotomy@localhost".into() }
    }
}

pub struct Store {
    pub(crate) settings: UserSettings,
    pub(crate) inner: Arc<jj_lib::store::Store>,
    git_dir: PathBuf,
}

/// A candidate placed on top of an integration version (harness-adapter.md §4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composed {
    pub commit: String,
    /// Paths jj recorded as conflicted. Empty when the result is clean.
    pub conflicts: Vec<String>,
}

pub(crate) fn settings() -> Result<UserSettings> {
    let mut config = StackedConfig::with_defaults();
    // Conflict markers in files use git's style, which agents and users already know. Commits
    // carry no jj headers, so integration versions are plain git commits in the user's repo.
    let layer = ConfigLayer::parse(
        ConfigSource::User,
        r#"
        user.name = "Lobotomy"
        user.email = "lobotomy@localhost"
        ui.conflict-marker-style = "git"
        git.write-change-id-header = false
        "#,
    )
    .map_err(Error::jj)?;
    config.add_layer(layer);
    UserSettings::from_config(config).map_err(Error::jj)
}

impl Store {
    /// Creates the store in `root`, which must not exist yet.
    pub fn init(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root).map_err(Error::io(root))?;
        let settings = settings()?;
        let backend =
            GitBackend::init_internal(&settings, root, gix_sha1()).map_err(Error::jj)?;
        Self::with_backend(settings, backend)
    }

    pub fn open(root: &Path) -> Result<Self> {
        let settings = settings()?;
        let backend = GitBackend::load(&settings, root).map_err(Error::jj)?;
        Self::with_backend(settings, backend)
    }

    /// Opens the store, creating it on first use.
    pub fn open_or_init(root: &Path) -> Result<Self> {
        if root.join("git_target").exists() { Self::open(root) } else { Self::init(root) }
    }

    fn with_backend(settings: UserSettings, backend: GitBackend) -> Result<Self> {
        let git_dir = backend.git_repo_path().to_path_buf();
        let merge_options = MergeOptions::from_settings(&settings).map_err(Error::jj)?;
        let inner = jj_lib::store::Store::new(Box::new(backend), Signer::new(None, vec![]), merge_options);
        Ok(Store { settings, inner, git_dir })
    }

    /// The store's bare git repository. Materialized directories borrow its objects.
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Copies `branch` of another repository into the store and pins it. Returns the commit,
    /// which must equal `expected` when given: the repository may have moved since it was checked.
    pub fn import(&self, repo: &Path, branch: &str, expected: Option<&str>, pin: &str) -> Result<String> {
        let git = Git::bare(&self.git_dir);
        let refspec = format!("+refs/heads/{branch}:refs/lobotomy/import");
        git.run(&["fetch", "--no-tags", "--quiet", &path_arg(repo), &refspec])?;
        let commit = git.run(&["rev-parse", "--verify", "refs/lobotomy/import^{commit}"])?;
        if let Some(expected) = expected
            && expected != commit
        {
            return Err(Error::Invalid(format!("{branch} moved to {commit} while it was imported (expected {expected})")));
        }
        // Reading the commit through jj imports it, assigning it a change id.
        self.commit(&commit)?;
        self.pin(pin, &commit)?;
        Ok(commit)
    }

    pub fn pin(&self, name: &str, commit: &str) -> Result<()> {
        Git::bare(&self.git_dir).run(&["update-ref", &format!("{PIN_NAMESPACE}{name}"), commit])?;
        Ok(())
    }

    /// The commit pinned under `name`, if any.
    pub fn pinned(&self, name: &str) -> Result<Option<String>> {
        Git::bare(&self.git_dir).optional(&["rev-parse", "-q", "--verify", &format!("{PIN_NAMESPACE}{name}^{{commit}}")])
    }

    /// Points a ref of the store at `commit`, for other repositories to fetch.
    pub(crate) fn export(&self, name: &str, commit: &str) -> Result<()> {
        Git::bare(&self.git_dir).run(&["update-ref", name, commit])?;
        Ok(())
    }

    pub(crate) fn commit(&self, hex: &str) -> Result<Commit> {
        let id = CommitId::try_from_hex(hex).ok_or_else(|| Error::Invalid(format!("not a commit id: {hex}")))?;
        self.inner.get_commit(&id).map_err(Error::jj)
    }

    pub(crate) fn write_commit(&self, parent: &Commit, tree: MergedTree, message: &str, who: &Identity) -> Result<String> {
        let signature = Signature { name: who.name.clone(), email: who.email.clone(), timestamp: Timestamp::now() };
        let (root_tree, labels) = tree.into_tree_ids_and_labels();
        let commit = backend::Commit {
            parents: vec![parent.id().clone()],
            predecessors: vec![],
            root_tree,
            conflict_labels: labels.into_merge(),
            change_id: self.settings.get_rng().new_change_id(self.inner.change_id_length()),
            description: message.to_owned(),
            author: signature.clone(),
            committer: signature,
            secure_sig: None,
        };
        let written = self.inner.write_commit(commit, None).block_on().map_err(Error::jj)?;
        Ok(written.id().hex())
    }

    /// Places `candidate` on top of `onto`: the candidate's changes relative to its own parent are
    /// merged into `onto`'s tree. Conflicts do not stop the rebase; jj records them in the commit
    /// (harness-adapter.md §4.2). The result has `onto` as its only parent and is pinned under
    /// `pin`; asking again with the same pin returns the same commit.
    pub fn compose(&self, candidate: &str, onto: &str, message: &str, author: &Identity, pin: &str) -> Result<Composed> {
        if let Some(existing) = self.pinned(pin)? {
            let conflicts = conflicts(&self.commit(&existing)?.tree());
            return Ok(Composed { commit: existing, conflicts });
        }
        let candidate = self.commit(candidate)?;
        let onto = self.commit(onto)?;
        let [parent] = candidate.parent_ids() else {
            return Err(Error::Invalid(format!("{} does not have exactly one parent", candidate.id().hex())));
        };
        let tree = if parent == onto.id() {
            candidate.tree()
        } else {
            let base = self.inner.get_commit(parent).map_err(Error::jj)?;
            MergedTree::merge(Merge::from_vec(vec![
                (onto.tree(), "integration".to_owned()),
                (base.tree(), "baseline".to_owned()),
                (candidate.tree(), "candidate".to_owned()),
            ]))
            .block_on()
            .map_err(Error::jj)?
        };
        let conflicts = conflicts(&tree);
        let commit = self.write_commit(&onto, tree, message, author)?;
        self.pin(pin, &commit)?;
        Ok(Composed { commit, conflicts })
    }

    /// The commit's parent.
    pub fn parent(&self, commit: &str) -> Result<String> {
        let commit = self.commit(commit)?;
        match commit.parent_ids() {
            [parent] => Ok(parent.hex()),
            _ => Err(Error::Invalid(format!("{} does not have exactly one parent", commit.id().hex()))),
        }
    }

    /// Whether two commits have the same content.
    pub fn same_tree(&self, a: &str, b: &str) -> Result<bool> {
        Ok(self.commit(a)?.tree().tree_ids() == self.commit(b)?.tree().tree_ids())
    }
}

/// One side of a changed file, for display.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Text { text: String },
    Binary { size: u64 },
    TooLarge { size: u64 },
    Symlink { target: String },
    /// jj recorded a conflict at this path.
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FileChange {
    pub path: String,
    /// `None`: the file did not exist on that side.
    pub old: Option<Content>,
    pub new: Option<Content>,
}

/// Files larger than this are listed without their content.
const DIFF_CONTENT_LIMIT: u64 = 2 * 1024 * 1024;

impl Store {
    /// The files that differ between two commits, with both sides' content.
    pub fn diff(&self, from: &str, to: &str) -> Result<Vec<FileChange>> {
        use futures::StreamExt as _;
        let (from, to) = (self.commit(from)?.tree(), self.commit(to)?.tree());
        async {
            let mut changes = Vec::new();
            let mut stream = from.diff_stream(&to, &jj_lib::matchers::EverythingMatcher);
            while let Some(entry) = stream.next().await {
                let values = entry.values.map_err(Error::jj)?;
                let old = self.content(&entry.path, &values.before).await?;
                let new = self.content(&entry.path, &values.after).await?;
                changes.push(FileChange { path: entry.path.as_internal_file_string().to_owned(), old, new });
            }
            Ok(changes)
        }
        .block_on()
    }

    async fn content(
        &self,
        path: &jj_lib::repo_path::RepoPath,
        value: &jj_lib::backend::MergedTreeValue,
    ) -> Result<Option<Content>> {
        use futures::io::AsyncReadExt as _;
        use jj_lib::backend::TreeValue;
        let Some(value) = value.as_resolved() else {
            return Ok(Some(Content::Conflict));
        };
        Ok(match value {
            None => None,
            Some(TreeValue::File { id, .. }) => {
                let mut reader = self.inner.read_file(path, id).await.map_err(Error::jj)?;
                let mut bytes = Vec::new();
                (&mut reader).take(DIFF_CONTENT_LIMIT + 1).read_to_end(&mut bytes).await.map_err(Error::io(path.as_internal_file_string()))?;
                let size = bytes.len() as u64;
                if size > DIFF_CONTENT_LIMIT {
                    Some(Content::TooLarge { size })
                } else if bytes.contains(&0) {
                    Some(Content::Binary { size })
                } else {
                    match String::from_utf8(bytes) {
                        Ok(text) => Some(Content::Text { text }),
                        Err(_) => Some(Content::Binary { size }),
                    }
                }
            }
            Some(TreeValue::Symlink(id)) => {
                Some(Content::Symlink { target: self.inner.read_symlink(path, id).await.map_err(Error::jj)? })
            }
            Some(_) => None,
        })
    }
}

fn conflicts(tree: &MergedTree) -> Vec<String> {
    tree.conflicts().map(|(path, _)| path.as_internal_file_string().to_owned()).collect()
}

fn gix_sha1() -> gix::hash::Kind {
    gix::hash::Kind::Sha1
}

/// A path as a git argument. Forward slashes keep git from reading `\` sequences on Windows.
pub(crate) fn path_arg(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
