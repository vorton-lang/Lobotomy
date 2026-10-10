//! Process ownership: both explicit completion and cancellation cover ordinary descendants.
use std::path::Path;
use std::time::Duration;

use lobotomy_harness::process::{Spawned, Spec, is_running, spawn};
use tokio::io::{AsyncBufReadExt, BufReader};

async fn ready_tree(dir: &Path) -> (Spawned, u32) {
    let script = dir.join("tree.mjs");
    std::fs::write(
        &script,
        r#"import { fork } from 'node:child_process';
process.on('SIGINT', () => {});
if (process.argv[2] === 'child') {
  process.send(process.pid);
  setInterval(() => {}, 1000);
} else {
  const child = fork(process.argv[1], ['child'], { stdio: ['ignore', 'ignore', 'ignore', 'ipc'] });
  child.once('message', pid => process.stdout.write(`${pid}\n`));
  setInterval(() => {}, 1000);
}
"#,
    )
    .unwrap();
    let args = [script.to_string_lossy().into_owned()];
    let mut tree =
        spawn(&Spec { program: Path::new("node"), args: &args, cwd: dir, env: &[], env_remove: &[] }).unwrap();
    tree.resume().unwrap();
    let mut line = String::new();
    let mut output = BufReader::new(tree.child.stdout.take().unwrap());
    tokio::time::timeout(Duration::from_secs(10), output.read_line(&mut line)).await.unwrap().unwrap();
    let child = line.trim().parse().unwrap();
    assert!(is_running(tree.pid, tree.process_start));
    assert!(child_is_running(child));
    (tree, child)
}

fn child_is_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        stat.rsplit_once(')').is_some_and(|(_, rest)| !matches!(rest.split_whitespace().next(), Some("Z" | "X")))
    }
    #[cfg(windows)]
    {
        let out =
            std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    }
}

#[tokio::test]
async fn finish_confirms_the_cli_and_its_child_have_exited() {
    let dir = tempfile::tempdir().unwrap();
    let (tree, child) = ready_tree(dir.path()).await;
    let (pid, start) = (tree.pid, tree.process_start);
    tokio::time::timeout(Duration::from_secs(10), tree.finish()).await.unwrap().unwrap();
    assert!(!is_running(pid, start));
    assert!(!child_is_running(child));
}

#[tokio::test]
async fn dropping_the_owner_also_ends_the_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let (tree, child) = ready_tree(dir.path()).await;
    let (pid, start) = (tree.pid, tree.process_start);
    drop(tree);
    tokio::time::timeout(Duration::from_secs(10), async {
        while is_running(pid, start) || child_is_running(child) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropping the owner must terminate its processes");
}
