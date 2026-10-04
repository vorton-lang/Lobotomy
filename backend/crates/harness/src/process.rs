//! Starting, watching and stopping harness processes (harness-adapter.md §1.3 rule 6, §1.8;
//! data-model.md §3.3).
//!
//! The platform layer is thin. On Windows each turn's CLI runs in its own kill-on-close Job
//! Object and on its own hidden console:
//! - The process is created suspended and joins the job before it runs, so nothing it starts can
//!   escape the job.
//! - If the backend dies, the OS closes the job handle and ends the CLI with everything it
//!   started.
//! - After the CLI exits, the runtime closes the job with [`Spawned::reap`], which ends whatever
//!   the CLI left running. Capture then sees a workspace no process is writing to
//!   (harness-adapter.md §4.1).
//! - Its own console lets a helper process send Ctrl+C to this CLI only.
//!
//! On Linux the CLI gets `PR_SET_PDEATHSIG`; the Linux side is not tested yet (harness-adapter.md
//! §0: Linux issues are fixed before the release).

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::Stdio;

use tokio::process::{Child, Command};

/// What to start. The process inherits the backend's environment, minus `env_remove`, plus
/// `env`.
pub struct Spec<'a> {
    pub program: &'a Path,
    pub args: &'a [String],
    pub cwd: &'a Path,
    pub env: &'a [(String, String)],
    pub env_remove: &'a [&'a str],
}

/// A started CLI. On Windows it does not run until [`Spawned::resume`].
pub struct Spawned {
    pub child: Child,
    pub pid: u32,
    /// The OS start time of the process; with the pid it identifies the process across a
    /// backend restart (data-model.md §3.2).
    pub process_start: i64,
    guard: imp::Guard,
}

impl Spawned {
    /// Lets the process run.
    pub fn resume(&mut self) -> io::Result<()> {
        self.guard.resume(self.pid)
    }

    /// Ends whatever the exited CLI left running. Call it after the CLI has exited.
    pub fn reap(self) {
        drop(self.guard);
    }
}

/// Starts the CLI with piped stdio.
pub fn spawn(spec: &Spec<'_>) -> io::Result<Spawned> {
    let mut cmd = Command::new(spec.program);
    for name in spec.env_remove {
        cmd.env_remove(name);
    }
    cmd.args(spec.args)
        .current_dir(spec.cwd)
        .envs(spec.env.iter().map(|(k, v)| (OsStr::new(k), OsStr::new(v))))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    imp::prepare(&mut cmd);
    let mut child = cmd.spawn()?;
    let pid = child.id().ok_or_else(|| io::Error::other("the process exited before it started"))?;
    match imp::adopt(&child, pid) {
        Ok((guard, process_start)) => Ok(Spawned { child, pid, process_start, guard }),
        Err(e) => {
            // It never ran on Windows: it was still suspended.
            let _ = child.start_kill();
            Err(e)
        }
    }
}

/// Whether the process that had this pid and start time is still running.
pub fn is_running(pid: u32, process_start: i64) -> bool {
    imp::is_running(pid, process_start)
}

/// Ends the process if it is still the one identified by pid and start time. Used for the
/// user's "终止残留进程" (data-model.md §3.3). Returns whether a process was ended.
pub fn terminate(pid: u32, process_start: i64) -> io::Result<bool> {
    imp::terminate(pid, process_start)
}

/// Asks the CLI to stop the way a user's Ctrl+C would (harness-adapter.md §1.3 rule 6). On
/// Windows a helper process sends it: `helper` is the program and arguments to which the pid is
/// appended, and the helper calls [`send_ctrl_c`].
pub async fn interrupt(pid: u32, helper: &[String]) -> io::Result<()> {
    imp::interrupt(pid, helper).await
}

/// The helper's side of [`interrupt`] on Windows: attaches to the CLI's console and sends Ctrl+C
/// there. The console is process-wide state, so this runs in a short-lived helper process, never
/// in the backend.
#[cfg(windows)]
pub fn send_ctrl_c(pid: u32) -> io::Result<()> {
    imp::send_ctrl_c(pid)
}

/// Makes the backend's own stdin, stdout and stderr non-inheritable. Without this every CLI
/// would inherit them, and whoever reads the backend's output would wait for the CLIs too
/// (harness-adapter.md §1.8). Call once at startup.
pub fn isolate_std_handles() {
    imp::isolate_std_handles();
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::ptr::null;

    use tokio::process::{Child, Command};
    use windows_sys::Win32::Foundation::{
        CloseHandle, FILETIME, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Console::{
        AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent, GetStdHandle, STD_ERROR_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, GetProcessTimes, OpenProcess, OpenThread,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, ResumeThread, TerminateProcess,
        THREAD_SUSPEND_RESUME, WaitForSingleObject,
    };

    /// An owned kernel handle.
    struct Owned(HANDLE);

    impl Owned {
        fn new(handle: HANDLE) -> io::Result<Self> {
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                Err(io::Error::last_os_error())
            } else {
                Ok(Self(handle))
            }
        }
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    // A kernel handle may be used and closed from any thread.
    unsafe impl Send for Owned {}
    unsafe impl Sync for Owned {}

    fn check(ok: i32) -> io::Result<()> {
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// The turn's job. Closing the last handle ends every process in it.
    pub struct Guard {
        _job: Owned,
        suspended: bool,
    }

    impl Guard {
        pub fn resume(&mut self, pid: u32) -> io::Result<()> {
            if self.suspended {
                resume_main_thread(pid)?;
                self.suspended = false;
            }
            Ok(())
        }
    }

    pub fn prepare(cmd: &mut Command) {
        cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
    }

    pub fn adopt(child: &Child, _pid: u32) -> io::Result<(Guard, i64)> {
        let process = child.raw_handle().ok_or_else(|| io::Error::other("no process handle"))? as HANDLE;
        let job = kill_on_close_job()?;
        check(unsafe { AssignProcessToJobObject(job.0, process) })?;
        let start = creation_time(process)?;
        Ok((Guard { _job: job, suspended: true }, start))
    }

    fn kill_on_close_job() -> io::Result<Owned> {
        let job = Owned::new(unsafe { CreateJobObjectW(null(), null()) })?;
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        check(unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        })?;
        Ok(job)
    }

    /// A suspended new process has exactly one thread.
    fn resume_main_thread(pid: u32) -> io::Result<()> {
        let snapshot = Owned::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
        let mut entry: THREADENTRY32 = unsafe { zeroed() };
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut more = unsafe { Thread32First(snapshot.0, &mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = Owned::new(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) })?;
                if unsafe { ResumeThread(thread.0) } == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            more = unsafe { Thread32Next(snapshot.0, &mut entry) } != 0;
        }
        Err(io::Error::other(format!("no thread found for process {pid}")))
    }

    fn creation_time(process: HANDLE) -> io::Result<i64> {
        let mut times: [FILETIME; 4] = unsafe { zeroed() };
        let [created, exited, kernel, user] = &mut times;
        check(unsafe { GetProcessTimes(process, created, exited, kernel, user) })?;
        Ok(((created.dwHighDateTime as i64) << 32) | created.dwLowDateTime as i64)
    }

    /// Opens the process if it is still the one that started at `process_start`.
    fn open_same(pid: u32, process_start: i64, access: u32) -> Option<Owned> {
        let process = Owned::new(unsafe { OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }).ok()?;
        (creation_time(process.0).ok()? == process_start).then_some(process)
    }

    pub fn is_running(pid: u32, process_start: i64) -> bool {
        open_same(pid, process_start, PROCESS_SYNCHRONIZE)
            .is_some_and(|process| unsafe { WaitForSingleObject(process.0, 0) } == WAIT_TIMEOUT)
    }

    pub fn terminate(pid: u32, process_start: i64) -> io::Result<bool> {
        let Some(process) = open_same(pid, process_start, PROCESS_TERMINATE | PROCESS_SYNCHRONIZE) else {
            return Ok(false);
        };
        if unsafe { WaitForSingleObject(process.0, 0) } != WAIT_TIMEOUT {
            return Ok(false);
        }
        check(unsafe { TerminateProcess(process.0, 1) })?;
        Ok(true)
    }

    pub async fn interrupt(pid: u32, helper: &[String]) -> io::Result<()> {
        let (program, args) = helper.split_first().ok_or_else(|| io::Error::other("no interrupt helper"))?;
        let status = Command::new(program)
            .args(args)
            .arg(pid.to_string())
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await?;
        if status.success() { Ok(()) } else { Err(io::Error::other(format!("interrupt helper failed: {status}"))) }
    }

    pub fn send_ctrl_c(pid: u32) -> io::Result<()> {
        unsafe {
            FreeConsole();
            check(AttachConsole(pid))?;
            // Ignore the event ourselves; it goes to every process on the console.
            SetConsoleCtrlHandler(None, 1);
            let sent = GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0);
            FreeConsole();
            check(sent)
        }
    }

    pub fn isolate_std_handles() {
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = unsafe { GetStdHandle(which) };
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::io;

    use tokio::process::{Child, Command};

    pub struct Guard;

    impl Guard {
        pub fn resume(&mut self, _pid: u32) -> io::Result<()> {
            Ok(())
        }
    }

    pub fn prepare(cmd: &mut Command) {
        let parent = std::process::id() as libc::pid_t;
        // Ask the kernel to send SIGTERM when the thread that started us exits. If the backend
        // died before this ran, exit now (harness-adapter.md §1.8).
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }

    pub fn adopt(_child: &Child, pid: u32) -> io::Result<(Guard, i64)> {
        Ok((Guard, start_time(pid)?))
    }

    /// Field 22 of /proc/<pid>/stat: start time in clock ticks after boot.
    fn start_time(pid: u32) -> io::Result<i64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let after_name = stat.rsplit_once(')').map(|(_, rest)| rest).unwrap_or_default();
        after_name
            .split_whitespace()
            .nth(19)
            .and_then(|field| field.parse().ok())
            .ok_or_else(|| io::Error::other(format!("cannot read the start time of {pid}")))
    }

    pub fn is_running(pid: u32, process_start: i64) -> bool {
        start_time(pid).is_ok_and(|start| start == process_start)
    }

    pub fn terminate(pid: u32, process_start: i64) -> io::Result<bool> {
        if !is_running(pid, process_start) {
            return Ok(false);
        }
        if unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(true)
    }

    pub async fn interrupt(pid: u32, _helper: &[String]) -> io::Result<()> {
        if unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn isolate_std_handles() {
        // std opens its own descriptors with O_CLOEXEC and the CLI gets fresh pipes.
    }
}
