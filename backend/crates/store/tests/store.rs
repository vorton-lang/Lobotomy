//! The store against real git repositories.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lobotomy_store::repo::{self, Diverged};
use lobotomy_store::{Captured, Guard, Identity, Leave, Scope, Store, Uncovered, Workspace};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    user_repo: PathBuf,
    store: Store,
    head: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let user_repo = root.join("repo");
    fs::create_dir_all(user_repo.join("src")).unwrap();
    git(&user_repo, &["init", "--quiet", "--initial-branch", "main"]);
    git(&user_repo, &["config", "user.name", "Test User"]);
    git(&user_repo, &["config", "user.email", "user@example.com"]);
    git(&user_repo, &["config", "core.autocrlf", "false"]);
    fs::write(user_repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    fs::write(user_repo.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    fs::write(user_repo.join(".gitignore"), "target/\n").unwrap();
    git(&user_repo, &["add", "."]);
    git(&user_repo, &["commit", "--quiet", "-m", "initial"]);
    let head = git(&user_repo, &["rev-parse", "HEAD"]);

    let store = Store::init(&root.join("data/store")).unwrap();
    let imported = store.import(&user_repo, "main", Some(&head), "import").unwrap();
    assert_eq!(imported, head);
    Fixture { _dir: dir, root, user_repo, store, head }
}

impl Fixture {
    fn workspace(&self, name: &str) -> Workspace {
        Workspace::new(self.root.join("data/slots").join(name), self.root.join("data/state").join(name))
    }
}

fn scope() -> Scope {
    Scope { excluded: vec!["node_modules/".into()], force_tracked: vec![] }
}

const GUARD: Guard = Guard { max_new_files: 100, max_new_bytes: 1 << 20 };

fn capture(f: &Fixture, ws: &Workspace, base: &str, pin: &str) -> String {
    match ws.capture(&f.store, base, &scope(), Some(&GUARD), Leave::default(), pin).unwrap() {
        Captured::Pinned { commit } => commit,
        other => panic!("not captured: {other:?}"),
    }
}

#[test]
fn a_slot_is_a_git_clone_at_the_baseline() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();

    assert_eq!(fs::read_to_string(slot.path.join("a.txt")).unwrap(), "one\ntwo\nthree\n");
    assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]), f.head);
    assert_eq!(git(&slot.path, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(git(&slot.path, &["status", "--porcelain"]), "", "a fresh slot is clean");
    assert_eq!(git(&slot.path, &["config", "core.autocrlf"]), "false", "no line ending conversion");
    // The state jj keeps for the directory is outside it.
    assert!(!slot.path.join(".jj").exists());
}

#[test]
fn a_capture_is_the_working_tree_on_the_baseline() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::write(slot.path.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    fs::write(slot.path.join("new.txt"), "new\n").unwrap();
    fs::create_dir_all(slot.path.join("target")).unwrap();
    fs::write(slot.path.join("target/cache.bin"), "cache").unwrap();
    fs::create_dir_all(slot.path.join("node_modules/x")).unwrap();
    fs::write(slot.path.join("node_modules/x/index.js"), "x").unwrap();
    // The agent's own commits do not matter; the capture is the working tree.
    git(&slot.path, &["-c", "user.name=a", "-c", "user.email=a@b", "commit", "--quiet", "-am", "draft"]);

    let commit = capture(&f, &slot, &f.head, "cap_1");
    assert_eq!(f.store.parent(&commit).unwrap(), f.head);
    assert_eq!(f.store.pinned("cap_1").unwrap().as_deref(), Some(commit.as_str()));

    // Capturing the same pin again returns the same commit, whatever the directory holds now.
    fs::write(slot.path.join("later.txt"), "later\n").unwrap();
    assert_eq!(capture(&f, &slot, &f.head, "cap_1"), commit);

    let composed = f.store.compose(&commit, &f.head, "task\n\nbody\n", &Identity::runtime(), "ver_1").unwrap();
    assert!(composed.conflicts.is_empty());
    let check = f.workspace("verify");
    check.materialize(&f.store, &composed.commit, &f.head, "main", &scope()).unwrap();
    assert_eq!(fs::read_to_string(check.path.join("a.txt")).unwrap(), "one\nTWO\nthree\n");
    assert!(check.path.join("new.txt").exists());
    assert!(!check.path.join("later.txt").exists(), "not part of the capture");
    assert!(!check.path.join("target").exists(), "ignored by .gitignore");
    assert!(!check.path.join("node_modules").exists(), "excluded by the scope");
}

#[test]
fn files_tracked_at_the_baseline_are_captured_even_when_ignored_later() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::write(slot.path.join(".gitignore"), "target/\nsrc/\n").unwrap();
    fs::write(slot.path.join("src/lib.rs"), "pub fn g() {}\n").unwrap();
    fs::write(slot.path.join("src/new.rs"), "ignored\n").unwrap();
    let commit = capture(&f, &slot, &f.head, "cap_1");

    let check = f.workspace("verify");
    check.materialize(&f.store, &commit, &f.head, "main", &scope()).unwrap();
    assert_eq!(fs::read_to_string(check.path.join("src/lib.rs")).unwrap(), "pub fn g() {}\n");
    assert!(!check.path.join("src/new.rs").exists());
}

#[test]
fn forced_paths_are_captured_despite_ignore_rules() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::write(slot.path.join(".gitignore"), "*\n").unwrap();
    fs::write(slot.path.join("src/new.rs"), "forced\n").unwrap();
    let forced = Scope { excluded: vec![], force_tracked: vec!["src".into()] };
    let Captured::Pinned { commit } = slot.capture(&f.store, &f.head, &forced, Some(&GUARD), Leave::default(), "cap_1").unwrap() else {
        panic!("not captured");
    };
    let check = f.workspace("verify");
    check.materialize(&f.store, &commit, &f.head, "main", &scope()).unwrap();
    assert!(check.path.join("src/new.rs").exists());
}

#[test]
fn the_guardrail_stops_before_anything_is_written() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    for i in 0..3 {
        fs::write(slot.path.join(format!("gen{i}.txt")), "x".repeat(10)).unwrap();
    }
    let guard = Guard { max_new_files: 2, max_new_bytes: 1 << 20 };
    match slot.capture(&f.store, &f.head, &scope(), Some(&guard), Leave::default(), "cap_1").unwrap() {
        Captured::Oversized { files, total_bytes } => {
            assert_eq!(files.len(), 3);
            assert_eq!(total_bytes, 30);
        }
        other => panic!("expected oversized, got {other:?}"),
    }
    assert_eq!(f.store.pinned("cap_1").unwrap(), None);

    // The user may leave the new files out: changes to tracked files are still captured.
    fs::write(slot.path.join("a.txt"), "changed\n").unwrap();
    let leave = Leave { new_files: true, ..Leave::default() };
    let Captured::Pinned { commit } = slot.capture(&f.store, &f.head, &scope(), Some(&guard), leave, "cap_left").unwrap() else {
        panic!("not captured");
    };
    let check = f.workspace("left");
    check.materialize(&f.store, &commit, &f.head, "main", &scope()).unwrap();
    assert_eq!(fs::read_to_string(check.path.join("a.txt")).unwrap(), "changed\n");
    assert!(!check.path.join("gen0.txt").exists());

    // Once the files are ignored the next capture passes and leaves them out.
    fs::write(slot.path.join(".gitignore"), "target/\ngen*.txt\n").unwrap();
    let commit = capture(&f, &slot, &f.head, "cap_2");
    let check = f.workspace("verify");
    check.materialize(&f.store, &commit, &f.head, "main", &scope()).unwrap();
    assert!(!check.path.join("gen0.txt").exists());
}

#[test]
fn a_nested_repository_fails_closed() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::create_dir_all(slot.path.join("vendor/lib")).unwrap();
    fs::write(slot.path.join("vendor/lib/x.txt"), "x").unwrap();
    git(&slot.path.join("vendor/lib"), &["init", "--quiet"]);

    match slot.capture(&f.store, &f.head, &scope(), Some(&GUARD), Leave::default(), "cap_1").unwrap() {
        Captured::Uncovered { paths } => assert_eq!(paths, vec![Uncovered::NestedRepo("vendor/lib".into())]),
        other => panic!("expected uncovered, got {other:?}"),
    }
    assert_eq!(f.store.pinned("cap_1").unwrap(), None);

    // A nested repository under an ignored directory is no loss.
    fs::write(slot.path.join(".gitignore"), "target/\nvendor/\n").unwrap();
    capture(&f, &slot, &f.head, "cap_2");
}

#[test]
fn the_user_may_discard_what_cannot_be_captured() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::create_dir_all(slot.path.join("vendor/lib")).unwrap();
    git(&slot.path.join("vendor/lib"), &["init", "--quiet"]);
    fs::write(slot.path.join("kept.txt"), "kept\n").unwrap();
    let Captured::Pinned { commit } = slot.capture(&f.store, &f.head, &scope(), Some(&GUARD), Leave { uncovered: true, ..Leave::default() }, "cap_1").unwrap() else {
        panic!("not captured");
    };
    let check = f.workspace("verify");
    check.materialize(&f.store, &commit, &f.head, "main", &scope()).unwrap();
    assert!(check.path.join("kept.txt").exists());
    assert!(!check.path.join("vendor").exists());
}

#[test]
fn rematerializing_keeps_ignored_caches_and_drops_uncaptured_files() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::write(slot.path.join("a.txt"), "changed\n").unwrap();
    fs::create_dir_all(slot.path.join("target")).unwrap();
    fs::write(slot.path.join("target/cache.bin"), "cache").unwrap();
    let commit = capture(&f, &slot, &f.head, "cap_1");
    let next = f.store.compose(&commit, &f.head, "done", &Identity::runtime(), "ver_1").unwrap().commit;
    fs::write(slot.path.join("stray.txt"), "never captured").unwrap();

    slot.materialize(&f.store, &next, &next, "main", &scope()).unwrap();
    assert_eq!(fs::read_to_string(slot.path.join("a.txt")).unwrap(), "changed\n");
    assert!(slot.path.join("target/cache.bin").exists(), "ignored caches stay");
    assert!(!slot.path.join("stray.txt").exists());
    assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]), next);
    assert_eq!(git(&slot.path, &["status", "--porcelain"]), "");
}

#[test]
fn line_endings_are_captured_as_written() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::write(slot.path.join("a.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
    let commit = capture(&f, &slot, &f.head, "cap_1");
    let check = f.workspace("verify");
    check.materialize(&f.store, &commit, &f.head, "main", &scope()).unwrap();
    assert_eq!(fs::read(check.path.join("a.txt")).unwrap(), b"one\r\ntwo\r\nthree\r\n");

    // The slot's git agrees, whatever the system's core.autocrlf says: `git diff` shows the
    // change jj captured, and restoring a file through git writes the stored bytes back.
    assert!(git(&slot.path, &["diff", "--stat"]).contains("a.txt"));
    git(&slot.path, &["checkout", "--", "a.txt"]);
    assert_eq!(fs::read(slot.path.join("a.txt")).unwrap(), b"one\ntwo\nthree\n");
}

#[test]
fn a_directory_moved_away_is_rebuilt_whole() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    // The directory went to quarantine; its jj state stayed behind.
    fs::rename(&slot.path, f.root.join("quarantined")).unwrap();
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    assert!(slot.path.join("a.txt").exists() && slot.path.join("src/lib.rs").exists());
    assert_eq!(git(&slot.path, &["status", "--porcelain"]), "");
}

#[test]
fn materializing_again_after_an_interruption_finishes_the_job() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::write(slot.path.join("a.txt"), "changed\n").unwrap();
    fs::write(slot.path.join("added.txt"), "added\n").unwrap();
    capture(&f, &slot, &f.head, "cap_1");
    // A checkout back to the baseline stopped halfway: one file written, the other not removed.
    fs::write(slot.path.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    assert_eq!(fs::read_to_string(slot.path.join("a.txt")).unwrap(), "one\ntwo\nthree\n");
    assert!(!slot.path.join("added.txt").exists());
}

#[test]
fn checking_out_restores_files_modified_since_the_last_materialization() {
    let f = fixture();
    let check = f.workspace("verify");
    check.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    // A check command modifies a tracked file.
    fs::write(check.path.join("a.txt"), "scribbled\n").unwrap();
    check.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    assert_eq!(fs::read_to_string(check.path.join("a.txt")).unwrap(), "one\ntwo\nthree\n");
}

#[test]
fn a_conflicting_rebase_records_the_conflict_and_materializes_markers() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::write(slot.path.join("a.txt"), "one\nfirst\nthree\n").unwrap();
    let first = capture(&f, &slot, &f.head, "cap_1");
    let accepted = f.store.compose(&first, &f.head, "first", &Identity::runtime(), "ver_1").unwrap().commit;

    // Another candidate made on the old baseline touches the same line.
    let other = f.workspace("other");
    other.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::write(other.path.join("a.txt"), "one\nsecond\nthree\n").unwrap();
    fs::write(other.path.join("b.txt"), "b\n").unwrap();
    let second = capture(&f, &other, &f.head, "cap_2");
    let composed = f.store.compose(&second, &accepted, "second", &Identity::runtime(), "ver_2").unwrap();
    assert_eq!(composed.conflicts, vec!["a.txt".to_owned()]);
    assert_eq!(f.store.parent(&composed.commit).unwrap(), accepted);

    other.materialize(&f.store, &composed.commit, &accepted, "main", &scope()).unwrap();
    let text = fs::read_to_string(other.path.join("a.txt")).unwrap();
    assert!(text.contains("<<<<<<<") && text.contains("first") && text.contains("second"), "{text}");
    assert!(other.path.join("b.txt").exists());

    // Resolving the markers and capturing again gives a clean candidate on the new baseline.
    fs::write(other.path.join("a.txt"), "one\nfirst and second\nthree\n").unwrap();
    let resolved = capture(&f, &other, &accepted, "cap_3");
    let clean = f.store.compose(&resolved, &accepted, "second", &Identity::runtime(), "ver_3").unwrap();
    assert!(clean.conflicts.is_empty());
}

#[test]
fn the_preview_fast_forwards_the_users_branch() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::write(slot.path.join("new.txt"), "new\n").unwrap();
    let commit = capture(&f, &slot, &f.head, "cap_1");
    let author = repo::identity(&f.user_repo).unwrap().unwrap();
    let message = "实现 new\n\n加了 new.txt\n\nLobotomy-Task: task_1\nLobotomy-Role: Malkuth\n";
    let target = f.store.compose(&commit, &f.head, message, &author, "ver_1").unwrap().commit;

    assert_eq!(repo::fast_forward(&f.user_repo, &f.store, "main", &f.head, &target).unwrap(), None);
    assert_eq!(git(&f.user_repo, &["rev-parse", "HEAD"]), target);
    // Writing again, as after a lost receipt, finds the repository at the target.
    assert_eq!(repo::fast_forward(&f.user_repo, &f.store, "main", &f.head, &target).unwrap(), None);
    assert_eq!(fs::read_to_string(f.user_repo.join("new.txt")).unwrap(), "new\n");
    assert_eq!(git(&f.user_repo, &["log", "-1", "--format=%an <%ae>"]), "Test User <user@example.com>");
    assert_eq!(git(&f.user_repo, &["log", "-1", "--format=%(trailers:key=Lobotomy-Task,valueonly)"]), "task_1");
    // A plain git commit: no jj headers.
    assert!(!git(&f.user_repo, &["cat-file", "-p", &target]).contains("change-id"));
}

#[test]
fn the_preview_stops_when_the_user_changed_the_repository() {
    let f = fixture();
    let slot = f.workspace("worker");
    slot.materialize(&f.store, &f.head, &f.head, "main", &scope()).unwrap();
    fs::write(slot.path.join("new.txt"), "new\n").unwrap();
    let commit = capture(&f, &slot, &f.head, "cap_1");
    let target = f.store.compose(&commit, &f.head, "x", &Identity::runtime(), "ver_1").unwrap().commit;

    fs::write(f.user_repo.join("a.txt"), "edited by hand\n").unwrap();
    let diverged = repo::fast_forward(&f.user_repo, &f.store, "main", &f.head, &target).unwrap();
    assert_eq!(diverged, Some(Diverged::Dirty(vec!["a.txt".into()])));
    assert_eq!(git(&f.user_repo, &["rev-parse", "HEAD"]), f.head, "nothing written");
    assert_eq!(fs::read_to_string(f.user_repo.join("a.txt")).unwrap(), "edited by hand\n");

    git(&f.user_repo, &["checkout", "--quiet", "a.txt"]);
    git(&f.user_repo, &["checkout", "--quiet", "-b", "other"]);
    let diverged = repo::fast_forward(&f.user_repo, &f.store, "main", &f.head, &target).unwrap();
    assert!(matches!(diverged, Some(Diverged::Branch { .. })));
}
