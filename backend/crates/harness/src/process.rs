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
//! On Linux the CLI gets `PR_SET_PDEATHSIG`. Managed shell commands have a kill-on-drop process
//! group; native trials also adopt the separate process group created for the terminal's PTY.

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
    cmd.args(spec.args);
    start(cmd, spec, false)
}

/// Runs a command line through the platform's shell, `cmd.exe` on Windows and `sh` elsewhere,
/// in a process of its own like a CLI. For the project's check commands (harness-adapter.md
/// §4.2); `spec.program` and `spec.args` are ignored.
///
/// [`Spawned::reap`] ends everything the command started, on every platform: a check left
/// running would hold the output pipes open and keep writing to the verification site (#12). On
/// Windows the job does it; elsewhere the command gets its own process group, which is killed.
pub fn spawn_shell(command: &str, spec: &Spec<'_>) -> io::Result<Spawned> {
    #[cfg(windows)]
    let cmd = {
        let mut cmd = Command::new("cmd.exe");
        // `/s` makes cmd strip exactly the outer quotes and run the rest as typed.
        cmd.args(["/d", "/s", "/c"]).raw_arg(format!("\"{command}\""));
        cmd
    };
    #[cfg(not(windows))]
    let cmd = {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    };
    start(cmd, spec, true)
}

/// Opens an interactive native terminal for a trial. `spec.program` and `spec.args` are
/// ignored. The current executable must dispatch `trial-terminal <title> <command> <control>`
/// to [`run_terminal_helper`] before starting its normal backend runtime.
///
/// The returned child lives for as long as the terminal is open, including after the command
/// finishes. Its exit status describes the terminal, not the trial command. Windows creates
/// the helper suspended in its own visible console and adopts it into the normal Job before
/// [`Spawned::resume`]. Linux uses a dedicated xfce4-terminal or xterm process, never a shared
/// terminal server, and records the PTY's separate process group before running the command.
/// Linux startup asynchronously waits up to ten seconds for that acknowledgement. Launch on
/// the backend's long-lived runtime, not a short-lived thread: Linux PDEATHSIG follows the
/// spawning thread's lifetime.
pub async fn spawn_terminal(command: &str, title: &str, spec: &Spec<'_>) -> io::Result<Spawned> {
    if command.contains('\0') || title.contains('\0') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "terminal command and title cannot contain NUL"));
    }
    imp::spawn_terminal(command, title, spec).await
}

/// Entry point for the private terminal helper. It must run in a fresh process, before a
/// Tokio runtime or any backend state is opened. The command is shell code, for Windows
/// PowerShell on Windows and `sh` elsewhere (check commands use `cmd.exe`); the title and
/// control path are literal arguments. The caller should exit with the returned status.
pub fn run_terminal_helper(command: &str, title: &str, control: &Path) -> io::Result<i32> {
    imp::run_terminal_helper(command, title, control)
}

/// `own_group`: on Unix, the process leads a new process group that reaping kills. Harness CLIs
/// do not get one; they clean up after themselves (harness-adapter.md §1.8).
fn start(mut cmd: Command, spec: &Spec<'_>, own_group: bool) -> io::Result<Spawned> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    start_with_stdio(cmd, spec, own_group, false)
}

fn start_with_stdio(mut cmd: Command, spec: &Spec<'_>, own_group: bool, visible_console: bool) -> io::Result<Spawned> {
    for name in spec.env_remove {
        cmd.env_remove(name);
    }
    cmd.current_dir(spec.cwd).envs(spec.env.iter().map(|(k, v)| (OsStr::new(k), OsStr::new(v))));
    imp::prepare(&mut cmd, own_group, visible_console);
    let mut child = cmd.spawn()?;
    let pid = child.id().ok_or_else(|| io::Error::other("the process exited before it started"))?;
    match imp::adopt(&child, pid, own_group) {
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
    use std::path::Path;
    use std::process::Stdio;
    use std::ptr::null;

    use tokio::process::{Child, Command};
    use windows_sys::Win32::Foundation::{
        CloseHandle, FILETIME, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Console::{
        AttachConsole, CTRL_BREAK_EVENT, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent, GetStdHandle,
        STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetConsoleTitleW,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_CONSOLE, CREATE_NO_WINDOW, CREATE_SUSPENDED, GetProcessTimes, OpenProcess, OpenThread,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, ResumeThread, THREAD_SUSPEND_RESUME,
        TerminateProcess, WaitForSingleObject,
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

    /// Every process gets a job of its own; the job covers what `own_group` asks for elsewhere.
    pub fn prepare(cmd: &mut Command, _own_group: bool, visible_console: bool) {
        cmd.creation_flags(CREATE_SUSPENDED | if visible_console { CREATE_NEW_CONSOLE } else { CREATE_NO_WINDOW });
    }

    pub async fn spawn_terminal(command: &str, title: &str, spec: &super::Spec<'_>) -> io::Result<super::Spawned> {
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args(["trial-terminal", title, command, ""])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        super::start_with_stdio(cmd, spec, true, true)
    }

    pub fn run_terminal_helper(command: &str, title: &str, _control: &Path) -> io::Result<i32> {
        let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
        check(unsafe { SetConsoleTitleW(title.as_ptr()) })?;
        // Keep the supervisor alive when Ctrl+C interrupts the command in the shell. A custom
        // handler, unlike SetConsoleCtrlHandler(NULL, TRUE), is not inherited by the shell.
        check(unsafe { SetConsoleCtrlHandler(Some(terminal_ctrl_handler), 1) })?;
        // A backend launched by Electron normally has pipes or NUL for its standard handles.
        // Inheriting those would leave a visible but unusable console. Open the helper's new
        // console explicitly, without attaching/detaching the multithreaded backend itself.
        let input = std::fs::OpenOptions::new().read(true).write(true).open("CONIN$")?;
        let output = std::fs::OpenOptions::new().read(true).write(true).open("CONOUT$")?;
        let error = output.try_clone()?;
        let mut shell = terminal_shell(command);
        let status = shell.stdin(input).stdout(output).stderr(error).status()?;
        Ok(status.code().unwrap_or(1))
    }

    unsafe extern "system" fn terminal_ctrl_handler(event: u32) -> i32 {
        i32::from(event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT)
    }

    /// Windows PowerShell, which every Windows has. `-EncodedCommand` carries the command as one
    /// opaque argument: no quoting layer, and every line runs. `Bypass` applies to this process
    /// only; under the default policy, script shims such as npm.ps1 would not load. `-NoExit`
    /// keeps output visible and leaves a working prompt after the trial exits.
    fn terminal_shell(command: &str) -> std::process::Command {
        let mut shell = std::process::Command::new("powershell.exe");
        shell
            .args(["-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-NoExit", "-EncodedCommand"])
            .arg(encode_command(command));
        shell
    }

    /// Base64 of the UTF-16LE text, as `-EncodedCommand` expects.
    fn encode_command(command: &str) -> String {
        const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes: Vec<u8> = command.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
            for i in 0..4 {
                out.push(if i <= chunk.len() { ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
            }
        }
        out
    }

    pub fn adopt(child: &Child, _pid: u32, _own_group: bool) -> io::Result<(Guard, i64)> {
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::{Duration, Instant};

        #[test]
        fn terminal_shell_passes_the_command_as_one_encoded_argument() {
            // The example from PowerShell's own help, and each padding length.
            assert_eq!(encode_command("dir"), "ZABpAHIA");
            assert_eq!(encode_command("a"), "YQA=");
            assert_eq!(encode_command("ab"), "YQBiAA==");
            let shell = terminal_shell("dir");
            let args: Vec<_> = shell.get_args().map(|arg| arg.to_str().unwrap()).collect();
            assert_eq!(args.last(), Some(&"ZABpAHIA"));
        }

        #[test]
        fn terminal_helper_entry() {
            let Ok(command) = std::env::var("LOBOTOMY_TEST_TERMINAL_COMMAND") else { return };
            let title = "trial & echo unexpected > title-injected";
            std::process::exit(run_terminal_helper(&command, title, Path::new("")).unwrap());
        }

        #[tokio::test]
        async fn native_console_is_interactive_and_does_not_run_before_job_adoption() {
            let dir = tempfile::tempdir().unwrap();
            // Several lines: the file appears only if lines after the first run too.
            let command = "$v = @([Console]::IsInputRedirected, [Console]::IsOutputRedirected, [Console]::IsErrorRedirected)\n\
                Set-Content console.txt ($v -join ',')\n\
                Start-Sleep 60";
            let mut cmd = Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "process::imp::tests::terminal_helper_entry", "--nocapture"])
                .env("LOBOTOMY_TEST_TERMINAL_COMMAND", command)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let spec = super::super::Spec {
                program: Path::new("ignored"),
                args: &[],
                cwd: dir.path(),
                env: &[],
                env_remove: &[],
            };
            let mut spawned = super::super::start_with_stdio(cmd, &spec, true, true).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            assert!(!dir.path().join("console.txt").exists());
            spawned.resume().unwrap();
            let deadline = Instant::now() + Duration::from_secs(20);
            let output = loop {
                if let Ok(output) = std::fs::read_to_string(dir.path().join("console.txt"))
                    && !output.trim().is_empty()
                {
                    break output;
                }
                assert!(Instant::now() < deadline, "console helper never started");
                std::thread::sleep(Duration::from_millis(50));
            };
            assert_eq!(output.trim(), "False,False,False", "all streams must use the new console");
            assert!(!dir.path().join("title-injected").exists(), "the title must never be evaluated as shell code");
            assert!(spawned.child.try_wait().unwrap().is_none());
            spawned.child.start_kill().unwrap();
            spawned.child.wait().await.unwrap();
            spawned.reap();
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::io::{self, Read, Write};
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::Duration;

    use tokio::process::{Child, Command};

    /// Kills the process group on drop, if the process leads one.
    pub struct Guard {
        group: Option<libc::pid_t>,
        terminal_group: Option<libc::pid_t>,
    }

    impl Guard {
        pub fn resume(&mut self, _pid: u32) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(group) = self.terminal_group {
                // A PTY starts a separate session; it is not in the emulator's process group.
                unsafe { libc::killpg(group, libc::SIGKILL) };
            }
            if let Some(group) = self.group {
                // ESRCH means the group is already gone.
                unsafe { libc::killpg(group, libc::SIGKILL) };
            }
        }
    }

    pub fn prepare(cmd: &mut Command, own_group: bool, _visible_console: bool) {
        if own_group {
            cmd.process_group(0);
        }
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

    pub fn adopt(_child: &Child, pid: u32, own_group: bool) -> io::Result<(Guard, i64)> {
        // With `process_group(0)` the group id is the child's pid.
        Ok((Guard { group: own_group.then_some(pid as libc::pid_t), terminal_group: None }, start_time(pid)?))
    }

    const TERMINAL_START_TIMEOUT: Duration = Duration::from_secs(10);

    /// Short-lived handshake, in a private directory. The helper cannot execute user code
    /// until its PTY group has been adopted; a closed/timed-out socket means "do not run".
    struct TerminalControl(PathBuf);

    impl TerminalControl {
        fn new() -> io::Result<Self> {
            let dir = std::env::temp_dir().join(format!("lobotomy-terminal-{}", ulid::Ulid::generate()));
            std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
            Ok(Self(dir))
        }

        fn socket(&self) -> PathBuf {
            self.0.join("ready")
        }
    }

    impl Drop for TerminalControl {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.socket());
            let _ = std::fs::remove_dir(&self.0);
        }
    }

    fn terminal_command(program: &str, helper: &Path, title: &str, command: &str, control: &Path) -> Command {
        let mut cmd = Command::new(program);
        if program == "xfce4-terminal" {
            cmd.args(["--disable-server", "--title", title, "--execute"]);
        } else {
            cmd.args(["-T", title, "-e"]);
        }
        cmd.arg(helper).args(["trial-terminal", title, command]).arg(control);
        // Only the emulator sees these descriptors. Its helper and command use the PTY.
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        cmd
    }

    pub async fn spawn_terminal(command: &str, title: &str, spec: &super::Spec<'_>) -> io::Result<super::Spawned> {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "native trials require a graphical desktop"));
        }
        let control = TerminalControl::new()?;
        let listener = UnixListener::bind(control.socket())?;
        listener.set_nonblocking(true)?;
        let helper = std::env::current_exe()?;
        let mut spawned = None;
        for program in ["xfce4-terminal", "xterm"] {
            let cmd = terminal_command(program, &helper, title, command, &control.socket());
            match super::start_with_stdio(cmd, spec, true, false) {
                Ok(child) => {
                    spawned = Some(child);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }
        }
        let mut spawned = spawned.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "native trials need xfce4-terminal or xterm installed")
        })?;
        if let Err(error) = adopt_terminal_group(&listener, &mut spawned).await {
            let _ = spawned.child.start_kill();
            // Also closes the handshake, so an unadopted helper cannot start the command.
            return Err(error);
        }
        Ok(spawned)
    }

    async fn adopt_terminal_group(listener: &UnixListener, spawned: &mut super::Spawned) -> io::Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::UnixListener::from_std(listener.try_clone()?)?;
        tokio::time::timeout(TERMINAL_START_TIMEOUT, async {
            let mut stream = tokio::select! {
                ready = listener.accept() => ready?.0,
                status = spawned.child.wait() => {
                    return Err(io::Error::other(format!("native terminal exited before opening ({})", status?)));
                }
            };
            let mut identity = [0; 12];
            stream.read_exact(&mut identity).await?;
            let pid = u32::from_ne_bytes(identity[..4].try_into().unwrap());
            let started = i64::from_ne_bytes(identity[4..].try_into().unwrap());
            if pid <= 1 || start_time(pid)? != started || unsafe { libc::getpgid(pid as i32) } != pid as i32 {
                return Err(io::Error::other("terminal helper did not create a dedicated process group"));
            }
            spawned.guard.terminal_group = Some(pid as libc::pid_t);
            stream.write_all(b"1").await?;
            Ok(())
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "native terminal did not open within ten seconds"))?
    }

    extern "C" fn close_terminal_group(_signal: libc::c_int) {
        // Only installed in the single-threaded helper after verifying it leads this group.
        // Both operations are async-signal-safe. This also covers the backend dying: the
        // emulator receives PDEATHSIG, then the helper receives its own PDEATHSIG.
        unsafe { libc::killpg(libc::getpgrp(), libc::SIGKILL) };
    }

    pub fn run_terminal_helper(command: &str, _title: &str, control: &Path) -> io::Result<i32> {
        let pid = std::process::id();
        if unsafe { libc::getpgrp() } != pid as libc::pid_t {
            return Err(io::Error::other("terminal helper must lead its own PTY process group"));
        }
        let parent = unsafe { libc::getppid() };
        unsafe {
            libc::signal(libc::SIGHUP, close_terminal_group as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, close_terminal_group as *const () as libc::sighandler_t);
            // Ctrl+C belongs to the user's command, not its supervisor.
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(io::Error::last_os_error());
            }
            if parent == 1 || libc::getppid() != parent {
                return Err(io::Error::other("native terminal closed during startup"));
            }
        }
        let mut stream = UnixStream::connect(control)?;
        stream.set_read_timeout(Some(TERMINAL_START_TIMEOUT))?;
        stream.write_all(&pid.to_ne_bytes())?;
        stream.write_all(&start_time(pid)?.to_ne_bytes())?;
        let mut accepted = [0];
        stream.read_exact(&mut accepted)?;
        if accepted != *b"1" {
            return Err(io::Error::other("terminal startup was cancelled"));
        }
        drop(stream);
        // Passing one literal -c argument avoids shell interpolation by a wrapper. In
        // particular, quotes, dollars, backticks and newlines reach sh exactly once.
        let mut shell = std::process::Command::new("sh");
        shell.arg("-c").arg(command);
        unsafe {
            shell.pre_exec(move || {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != pid as libc::pid_t {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
        let status = shell.status()?;
        // Keep the PTY group leader alive while the user reads the output. Terminal --hold
        // would outlive this helper, losing its parent-death handler and retaining a stale
        // process-group id until the user eventually closed the window.
        println!("\nCommand exited ({status}). Press Enter to close this trial terminal.");
        io::stdout().flush()?;
        let _ = io::stdin().read_line(&mut String::new())?;
        Ok(status.code().unwrap_or(1))
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::fs::File;
        use std::os::fd::FromRawFd;
        use std::time::Instant;

        #[test]
        fn terminal_options_use_an_owned_window_and_literal_arguments() {
            let title = "trial & 'quoted' $(title)";
            let command = "printf '%s' '$not_expanded'; echo \"quoted\"\nexit 7";
            for program in ["xfce4-terminal", "xterm"] {
                let cmd =
                    terminal_command(program, Path::new("/backend with spaces"), title, command, Path::new("/control"));
                let args: Vec<_> = cmd.as_std().get_args().collect();
                let prefix = if program == "xfce4-terminal" {
                    vec!["--disable-server", "--title", title, "--execute"]
                } else {
                    vec!["-T", title, "-e"]
                };
                let mut expected = prefix;
                expected.extend(["/backend with spaces", "trial-terminal", title, command, "/control"]);
                assert_eq!(args, expected);
            }
        }

        // This test is also the re-executed single-process helper used by the PTY tests.
        #[test]
        fn terminal_helper_entry() {
            let Ok(command) = std::env::var("LOBOTOMY_TEST_TERMINAL_COMMAND") else { return };
            let control = std::env::var_os("LOBOTOMY_TEST_TERMINAL_CONTROL").unwrap();
            let result = run_terminal_helper(&command, "test", Path::new(&control));
            match result {
                Ok(code) => std::process::exit(code),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(99);
                }
            }
        }

        fn helper_in_pty(command: &str, control: &Path, cwd: &Path) -> (super::super::Spawned, File) {
            let (mut master, mut slave) = (-1, -1);
            assert_eq!(
                unsafe {
                    libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null())
                },
                0
            );
            let master = unsafe { File::from_raw_fd(master) };
            let slave = unsafe { File::from_raw_fd(slave) };
            let mut cmd = Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "process::imp::tests::terminal_helper_entry", "--nocapture"])
                .env("LOBOTOMY_TEST_TERMINAL_COMMAND", command)
                .env("LOBOTOMY_TEST_TERMINAL_CONTROL", control)
                .stdin(slave.try_clone().unwrap())
                .stdout(slave.try_clone().unwrap())
                .stderr(slave);
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let spec = super::super::Spec { program: Path::new("ignored"), args: &[], cwd, env: &[], env_remove: &[] };
            let child = super::super::start_with_stdio(cmd, &spec, false, false).unwrap();
            (child, master)
        }

        fn read_terminal(master: &File) -> String {
            use std::os::fd::AsRawFd;
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
            let mut output = Vec::new();
            let mut buffer = [0; 1024];
            while let Ok(count) = (&*master).read(&mut buffer) {
                if count == 0 {
                    break;
                }
                output.extend_from_slice(&buffer[..count]);
            }
            String::from_utf8_lossy(&output).into_owned()
        }

        #[tokio::test]
        async fn terminal_helper_reads_real_tty_input_without_reexpanding_command() {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("control");
            let listener = UnixListener::bind(&socket).unwrap();
            listener.set_nonblocking(true).unwrap();
            let command = "test -t 0 && test -t 1 && test -t 2 || exit 31; \
                           printf '%s\\n' '$literal `backticks` \"quotes\"'; \
                           IFS= read -r answer; printf '%s' \"$answer\" > answer; \
                           pwd > cwd; printf 'output-ok\\n'; exit 7";
            let (mut spawned, mut master) = helper_in_pty(command, &socket, dir.path());
            adopt_terminal_group(&listener, &mut spawned).await.unwrap();
            master.write_all(b"keyboard input $literal & spaces\n\n").unwrap();
            let status = tokio::time::timeout(Duration::from_secs(5), spawned.child.wait()).await.unwrap().unwrap();
            assert_eq!(status.code(), Some(7));
            assert_eq!(std::fs::read_to_string(dir.path().join("answer")).unwrap(), "keyboard input $literal & spaces");
            assert_eq!(std::fs::read_to_string(dir.path().join("cwd")).unwrap().trim(), dir.path().to_str().unwrap());
            let output = read_terminal(&master);
            assert!(output.contains("$literal `backticks` \"quotes\""), "{output}");
            assert!(output.contains("output-ok"), "{output}");
            spawned.reap();
        }

        #[tokio::test]
        async fn a_cancelled_terminal_handshake_never_runs_the_command() {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("control");
            let listener = UnixListener::bind(&socket).unwrap();
            listener.set_nonblocking(true).unwrap();
            let (mut spawned, _master) = helper_in_pty("touch must-not-run", &socket, dir.path());
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok((stream, _)) = listener.accept() {
                    drop(stream);
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            let status = tokio::time::timeout(Duration::from_secs(5), spawned.child.wait()).await.unwrap().unwrap();
            assert_eq!(status.code(), Some(99));
            assert!(!dir.path().join("must-not-run").exists());
            spawned.reap();
        }

        #[tokio::test]
        async fn reaping_the_terminal_kills_its_separate_pty_group() {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("control");
            let listener = UnixListener::bind(&socket).unwrap();
            listener.set_nonblocking(true).unwrap();
            let (mut spawned, _master) = helper_in_pty("sleep 60 & echo $! > child; wait", &socket, dir.path());
            adopt_terminal_group(&listener, &mut spawned).await.unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !dir.path().join("child").exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            let pid: u32 = std::fs::read_to_string(dir.path().join("child")).unwrap().trim().parse().unwrap();
            let started = start_time(pid).unwrap();
            spawned.child.start_kill().unwrap();
            spawned.child.wait().await.unwrap();
            spawned.reap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
                let zombie = stat.rsplit_once(')').is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'));
                if !is_running(pid, started) || zombie {
                    break;
                }
                assert!(Instant::now() < deadline, "PTY descendant survived terminal cleanup");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
