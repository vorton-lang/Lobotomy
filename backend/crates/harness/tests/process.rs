//! The Windows platform layer with real processes (harness-adapter.md §1.8).
#![cfg(windows)]

use std::path::Path;
use std::time::{Duration, Instant};

use lobotomy_harness::process::{Spec, is_running, spawn, terminate};

fn powershell(script: &str, cwd: &Path) -> lobotomy_harness::process::Spawned {
    let args = ["-NoProfile", "-NonInteractive", "-Command", script].map(String::from);
    spawn(&Spec { program: Path::new("powershell.exe"), args: &args, cwd, env: &[] }).unwrap()
}

fn pid_alive(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
}

fn wait_until(mut condition: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[tokio::test]
async fn the_process_waits_until_resumed() {
    let dir = tempfile::tempdir().unwrap();
    let mut spawned = powershell("Set-Content started.txt 'yes'; Start-Sleep 30", dir.path());
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!dir.path().join("started.txt").exists(), "a suspended process must not run");
    spawned.resume().unwrap();
    assert!(wait_until(|| dir.path().join("started.txt").exists(), Duration::from_secs(20)));
    assert!(is_running(spawned.pid, spawned.process_start));
    assert!(!is_running(spawned.pid, spawned.process_start + 1), "another start time is another process");
    assert!(terminate(spawned.pid, spawned.process_start).unwrap());
    spawned.child.wait().await.unwrap();
    assert!(!is_running(spawned.pid, spawned.process_start));
    assert!(!terminate(spawned.pid, spawned.process_start).unwrap());
}

#[tokio::test]
async fn reaping_ends_what_the_cli_left_running() {
    let dir = tempfile::tempdir().unwrap();
    // The "CLI" starts a long-lived grandchild, records its pid and exits.
    let script = "$p = Start-Process ping.exe -ArgumentList '-n','60','127.0.0.1' -PassThru -WindowStyle Hidden; \
                  Set-Content grandchild.txt $p.Id";
    let mut spawned = powershell(script, dir.path());
    spawned.resume().unwrap();
    let status = spawned.child.wait().await.unwrap();
    assert!(status.success());
    let grandchild: u32 =
        std::fs::read_to_string(dir.path().join("grandchild.txt")).unwrap().trim().parse().unwrap();
    assert!(pid_alive(grandchild), "the grandchild outlives the CLI until reaped");
    spawned.reap();
    assert!(wait_until(|| !pid_alive(grandchild), Duration::from_secs(10)), "reaping ends the grandchild");
}
