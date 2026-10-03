//! Windows process-control probe for the harness adapter (notes/harness-adapter.md §1.8, §6).
//! Launches a command, waits, then ends it in one of four ways so we can observe what the
//! harness and its tool subprocesses do.
//!
//! Usage: win-proc <mode> <secs> <stdout-file> -- <cmd> [args...]
//!   job-crash    child in a KILL_ON_JOB_CLOSE job; this process terminates itself (simulated backend crash)
//!   no-job-kill  TerminateProcess on the child only (what a plain kill does)
//!   ctrl-break   child in its own process group; send CTRL_BREAK_EVENT to that group
//!   ctrl-c       send CTRL_C_EVENT to every process on our private console
//!   ctrl-c-isolated  child gets its own windowless console; attach to it and send CTRL_C_EVENT
//!                there only. A bystander heartbeat on another console must keep running.
//! ctrl-break and ctrl-c run on a fresh hidden console so the caller's console is never signalled.
use std::fs::File;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, ptr, thread};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Console::{
    AllocConsole, AttachConsole, CTRL_BREAK_EVENT, CTRL_C_EVENT, FreeConsole,
    GenerateConsoleCtrlEvent, GetConsoleWindow, SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, GetCurrentProcess, TerminateProcess,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};

fn main() {
    let args: Vec<String> = env::args().collect();
    let sep = args.iter().position(|a| a == "--").expect("missing --");
    let (mode, secs, out) = (args[1].as_str(), args[2].parse::<u64>().unwrap(), &args[3]);
    let t0 = Instant::now();
    let at = || format!("{:.1}s", t0.elapsed().as_secs_f64());

    if mode == "ctrl-break" || mode == "ctrl-c" {
        // A private console: console control events then reach only us and our children.
        unsafe {
            FreeConsole();
            AllocConsole();
            ShowWindow(GetConsoleWindow(), SW_HIDE);
        }
    }

    let mut cmd = Command::new(&args[sep + 1]);
    cmd.args(&args[sep + 2..])
        .stdin(Stdio::null())
        .stdout(File::create(out).unwrap())
        .stderr(File::create(format!("{out}.err")).unwrap());
    if mode == "ctrl-break" {
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    let mut bystander = None;
    if mode == "ctrl-c-isolated" {
        cmd.creation_flags(CREATE_NO_WINDOW);
        let hb = "let n=0;const t=setInterval(()=>{require('fs').appendFileSync('hb-bystander.txt',Date.now()+'\\n');if(++n==60)clearInterval(t)},1000)";
        let b = Command::new("node")
            .args(["-e", hb])
            .creation_flags(CREATE_NO_WINDOW)
            // Must not inherit our stdout: the caller would wait for the bystander to exit.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        println!("bystander_pid={}", b.id());
        bystander = Some(b);
    }

    let job: HANDLE = if mode == "job-crash" { kill_on_close_job() } else { ptr::null_mut() };
    let mut child = cmd.spawn().unwrap();
    // Spawn-then-assign leaves a short window before the child joins the job. A backend should
    // create the process suspended (or use PROC_THREAD_ATTRIBUTE_JOB_LIST) to close it.
    if !job.is_null() {
        let ok = unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) };
        println!("assigned_to_job={ok}");
    }
    println!("child_pid={}", child.id());
    thread::sleep(Duration::from_secs(secs));

    match mode {
        "job-crash" => {
            // Die the way a killed backend does: no destructors, no cleanup, handles closed by the OS.
            println!("self_terminate_at={}", at());
            unsafe { TerminateProcess(GetCurrentProcess(), 99) };
        }
        "no-job-kill" => {
            child.kill().unwrap();
            println!("terminated_at={}", at());
        }
        "ctrl-break" => {
            let ok = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) };
            println!("ctrl_break_sent={ok} at={}", at());
        }
        "ctrl-c" => {
            // Set after spawning: the ignore flag is inherited by children created later.
            let ok = unsafe {
                SetConsoleCtrlHandler(None, 1);
                GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)
            };
            println!("ctrl_c_sent={ok} at={}", at());
        }
        "ctrl-c-isolated" => {
            // Borrow the child's console just long enough to signal it. A backend would do this
            // from a short-lived helper process, because the console is process-wide state.
            let (attached, sent) = unsafe {
                FreeConsole();
                let attached = AttachConsole(child.id());
                SetConsoleCtrlHandler(None, 1);
                let sent = GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0);
                FreeConsole();
                (attached, sent)
            };
            println!("attached={attached} ctrl_c_sent={sent} at={}", at());
        }
        _ => panic!("unknown mode {mode}"),
    }
    drop(bystander);

    // Give the child up to 30s to exit on its own after a console event.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            println!("child_exit_code={:?} at={}", status.code(), at());
            break;
        }
        if Instant::now() > deadline {
            println!("child_still_running_after_30s");
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn kill_on_close_job() -> HANDLE {
    unsafe {
        let job = CreateJobObjectW(ptr::null(), ptr::null());
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        assert!(ok != 0, "SetInformationJobObject failed");
        job
    }
}
