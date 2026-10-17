use std::path::Path;
use std::process::Command;

use lobotomy_worker::environment::{AttemptDirs, Environment, Repo};

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

fn environment() -> (tempfile::TempDir, Environment, AttemptDirs) {
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream");
    std::fs::create_dir(&upstream).unwrap();
    git(&upstream, &["init", "--initial-branch=main"]);
    std::fs::create_dir(upstream.join("notes")).unwrap();
    std::fs::write(upstream.join("notes/protected.md"), "protected").unwrap();
    git(&upstream, &["add", "."]);
    git(
        &upstream,
        &["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "--allow-empty", "-m", "base"],
    );
    let root = dir.path().join("worker");
    std::fs::create_dir(&root).unwrap();
    let environment = Environment::new(
        &root,
        Repo {
            upstream: upstream.to_str().unwrap().to_owned(),
            base: "main".into(),
            author_name: "Test".into(),
            author_email: "test@example.com".into(),
            port_start: 42000,
        },
    );
    let task_dir = root.join("task");
    std::fs::create_dir(&task_dir).unwrap();
    let dirs = AttemptDirs::new(&task_dir, 1);
    environment.prepare(&dirs, "lobotomy/task-1", &[]).unwrap();
    (dir, environment, dirs)
}

#[test]
fn delivered_paths_preserve_the_names_used_for_authorization() {
    let (_dir, environment, dirs) = environment();
    git(&dirs.clone, &["config", "core.quotePath", "true"]);
    let mut names = vec!["notes/中文.md", "notes/space name.md"];
    // Windows does not permit these names, but git's quoted output must not hide them on Unix.
    #[cfg(unix)]
    names.extend(["notes/tab\tname.md", "notes/line\nname.md", "notes/quote\"name.md", "notes/trailing "]);
    for name in &names {
        std::fs::write(dirs.clone.join(name), "changed").unwrap();
    }

    let delivered = environment.deliver(&dirs, "refs/lobotomy/task-1").unwrap().unwrap();
    let mut files = delivered.files;
    files.sort();
    names.sort();
    assert_eq!(files, names);
    assert!(files.iter().all(|path| path.starts_with("notes/")));
}

#[test]
fn moving_a_protected_file_out_of_its_directory_keeps_both_paths() {
    let (_dir, environment, dirs) = environment();
    git(&dirs.clone, &["config", "diff.renames", "true"]);
    std::fs::rename(dirs.clone.join("notes/protected.md"), dirs.clone.join("moved.md")).unwrap();

    let delivered = environment.deliver(&dirs, "refs/lobotomy/task-1").unwrap().unwrap();
    let mut files = delivered.files;
    files.sort();
    assert_eq!(files, ["moved.md", "notes/protected.md"]);
}
