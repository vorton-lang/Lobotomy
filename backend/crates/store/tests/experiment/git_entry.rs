//! Temporary opt-in experiment. Included in the *same* integration-test executable as the fixtures.
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

fn command(store_flags: bool) -> Command {
    let mut command = Command::new(std::env::var_os("LOBOTOMY_EXPERIMENT_PROGRAM").unwrap_or_else(|| "git".into()));
    if store_flags {
        command.env("GIT_TERMINAL_PROMPT", "0");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // The store's CREATE_NO_WINDOW.
        }
    }
    command
}

#[cfg(windows)]
fn image(store_flags: bool) -> PathBuf {
    use std::ffi::c_void;
    use std::os::windows::{io::AsRawHandle, process::CommandExt};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryFullProcessImageNameW(process: *mut c_void, flags: u32, name: *mut u16, size: *mut u32) -> i32;
    }
    let flags = 0x0000_0004 | if store_flags { 0x0800_0000 } else { 0 }; // CREATE_SUSPENDED
    let mut child = command(store_flags)
        .arg("--version")
        .creation_flags(flags)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn image probe");
    let mut buffer = vec![0u16; 32768];
    let mut size = buffer.len() as u32;
    // The live Child owns the process handle. The writable buffer contains `size` UTF-16 units.
    let ok = unsafe { QueryFullProcessImageNameW(child.as_raw_handle(), 0, buffer.as_mut_ptr(), &mut size) };
    let error = std::io::Error::last_os_error();
    // Clean up before asserting, including the query-failure path. The probe never resumes.
    let killed = child.kill();
    let waited = child.wait();
    killed.expect("terminate suspended probe");
    waited.expect("reap suspended probe");
    assert_ne!(ok, 0, "query process image: {error}");
    PathBuf::from(String::from_utf16(&buffer[..size as usize]).expect("UTF-16 image path"))
}

// Local orchestration validation only; Windows evidence uses the process handle above.
#[cfg(not(windows))]
fn image(store_flags: bool) -> PathBuf {
    let mut child = command(store_flags)
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let path = std::fs::read_link(format!("/proc/{}/exe", child.id()));
    child.kill().unwrap();
    child.wait().unwrap();
    path.unwrap()
}

fn opt_in() {
    assert_eq!(std::env::var("LOBOTOMY_GIT_EXPERIMENT").as_deref(), Ok("1"));
}

#[test]
#[ignore = "temporary Windows Git entry experiment; explicit opt-in only"]
fn probe() {
    opt_in();
    for (name, store_flags) in [("fixture", false), ("store", true)] {
        let path = image(store_flags);
        assert!(path.is_absolute());
        println!("\nGIT_EXPERIMENT\timage_{name}\t{}", path.display());
    }
    let out = command(false).arg("--version").output().unwrap();
    assert!(out.status.success());
    println!("\nGIT_EXPERIMENT\tversion\t{}", String::from_utf8(out.stdout).unwrap().trim());
}

#[test]
#[ignore = "temporary Windows Git entry experiment; explicit opt-in only"]
fn version_bench() {
    opt_in();
    // Sequential samples, spawn-to-wait including output capture; no suspended probes in the timer.
    for index in 0..10 {
        let start = Instant::now();
        let out = command(false).arg("--version").output().unwrap();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        assert!(out.status.success());
        assert!(String::from_utf8(out.stdout).unwrap().starts_with("git version "));
        println!("\nGIT_EXPERIMENT\tversion_ms_{index}\t{elapsed:.6}");
    }
}
