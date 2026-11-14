//! Command resolution (`which` + `PATHEXT`), path normalization (`dunce`), and
//! process tree management via Windows Job Objects (`WinJobGuard`).
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §5.1–§5.2.
//!
//! - **Command resolution**: always go through [`resolve_command`] when launching
//!   `npx` / `uvx` / `opencode` etc. On Windows the extension (`PATHEXT`) of
//!   `opencode.cmd` / `npx.cmd` must be resolved before starting the process.
//! - **Avoiding UNC paths**: Windows' `std::fs::canonicalize` prepends the UNC
//!   prefix `\\?\D:\ghq\...`, which can make Node.js based agents fail to
//!   interpret paths. Always use [`canonicalize`] (`dunce`) instead.
//! - **Guaranteed process tree termination**: on Windows, child processes are
//!   bound to a **Job Object** so that even grandchildren are reliably terminated
//!   when the daemon exits ([`ProcessTreeGuard`]).
//! - **Clean exit of wrapper launchers**: wrappers such as `npx` → node /
//!   `uvx` → python / PyInstaller onefile "clean up after themselves and exit
//!   naturally once the inner process exits". Shutdown therefore terminates from
//!   the inside out and waits for the root to exit naturally before force-killing
//!   it ([`ProcessTreeGuard::shutdown_tree`]). The official ACP SDK also
//!   terminates whole process groups on Unix for the same reason.

use std::path::{Path, PathBuf};

#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
#[cfg(windows)]
use windows::Win32::System::JobObjects::{
    JOBOBJECT_BASIC_PROCESS_ID_LIST, JobObjectBasicProcessIdList, QueryInformationJobObject,
};
#[cfg(windows)]
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
    WaitForSingleObject,
};

use crate::error::PtyError;

/// Resolves a command from the given directory and `PATH` (`PATHEXT` aware).
///
/// On Windows this resolves extensions such as `.cmd` / `.exe` automatically
/// (`which::which_in` honors `PATHEXT`).
pub fn resolve_command(command: &str, cwd: impl AsRef<Path>) -> Result<PathBuf, PtyError> {
    let cwd = cwd.as_ref();
    match which::which_in(command, std::env::var_os("PATH"), cwd) {
        Ok(path) => Ok(path),
        Err(err) => Err(PtyError::CommandNotFound {
            command: command.to_owned(),
            cwd: cwd.to_path_buf(),
            reason: err.to_string(),
        }),
    }
}

/// Normalizes a path with `dunce::canonicalize`, which does not add the `\\?\`
/// UNC prefix.
pub fn canonicalize(path: impl AsRef<Path>) -> Result<PathBuf, PtyError> {
    let path = path.as_ref();
    dunce::canonicalize(path).map_err(|source| PtyError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Guard that reliably terminates a process tree per process group / Job Object.
///
/// - **Windows**: child processes are bound to a **Job Object** (`kill on job
///   close`). When the guard (and thus the daemon) exits, the Job Object handle is
///   closed and **the whole tree, including grandchildren, is reliably
///   terminated**.
/// - **Unix**: terminating the PTY session (session leader) terminates the child
///   process group, so nothing is done here (no additional state is kept).
pub struct ProcessTreeGuard {
    #[cfg(windows)]
    job: WinJobGuard,
}

/// Result of [`ProcessTreeGuard::shutdown_tree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeShutdownOutcome {
    /// Number of processes terminated other than the root.
    ///
    /// Always 0 on Unix, where descendants cannot be enumerated (known
    /// limitation).
    pub terminated_descendants: usize,
    /// Whether the root exited naturally within the grace period.
    ///
    /// Always `false` on Unix, where no waiting is performed.
    pub root_exited_naturally: bool,
}

impl std::fmt::Debug for ProcessTreeGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessTreeGuard")
            .field(
                "backend",
                &if cfg!(windows) { "job-object" } else { "none" },
            )
            .finish()
    }
}

impl ProcessTreeGuard {
    /// Creates a new guard (on Windows, creates a kill-on-close Job Object).
    pub fn new() -> Result<Self, PtyError> {
        #[cfg(windows)]
        {
            Ok(Self {
                job: WinJobGuard::new_kill_on_close()?,
            })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }

    /// Adds a child process to the guard (Windows: assign to the Job Object /
    /// Unix: no-op).
    ///
    /// A member process is terminated **together with its whole process tree**
    /// when the guard is dropped.
    pub fn attach(&self, child: &dyn portable_pty::Child) -> Result<(), PtyError> {
        #[cfg(windows)]
        {
            if let Some(handle) = child.as_raw_handle() {
                self.job.assign_process(handle)?;
            }
        }
        #[cfg(not(windows))]
        {
            // On Unix, leave termination to the PTY session (the process is bound
            // to the PTY as session leader and receives SIGHUP when the master
            // side is closed).
            let _ = child;
        }
        Ok(())
    }

    /// Assigns a Windows RawHandle to the Job Object.
    #[cfg(windows)]
    pub fn attach_raw_handle(
        &self,
        handle: std::os::windows::io::RawHandle,
    ) -> Result<(), PtyError> {
        self.job.assign_process(handle)
    }

    /// Adds a `std::process::Child` to the guard (Windows: assign to the Job
    /// Object / Unix: no-op).
    pub fn attach_std_child(&self, child: &std::process::Child) -> Result<(), PtyError> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            self.job.assign_process(child.as_raw_handle())?;
        }
        #[cfg(not(windows))]
        {
            let _ = child;
        }
        Ok(())
    }

    /// Terminates every Job member except the given PID (Windows only, best
    /// effort).
    ///
    /// Returns the number of processes terminated (already-exited ones are not
    /// counted).
    #[cfg(windows)]
    pub fn terminate_all_except(&self, keep_pid: u32) -> Result<usize, PtyError> {
        self.job.terminate_all_except(keep_pid)
    }

    /// Whether process tree management via Job Objects is enabled (diagnostic).
    pub fn is_job_object_backend(&self) -> bool {
        cfg!(windows)
    }
}

/// Process tree shutdown policy (Windows implementation).
///
/// Wrapper launchers (`npx` → node / `uvx` → python / PyInstaller onefile, etc.)
/// "clean up after themselves and exit naturally once the inner process exits".
#[cfg(windows)]
impl ProcessTreeGuard {
    /// Assigns the process with the given PID to the Job Object (making it a
    /// target of the tree kill on shutdown).
    ///
    /// Because the handle is reopened, callers do not need to keep a raw handle
    /// (`child.id()` is enough, so platform branching does not leak to callers).
    pub fn attach_pid(&self, pid: u32) -> Result<(), PtyError> {
        unsafe {
            let handle = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid)?;
            let result = self.job.assign_process(handle.0);
            let _ = CloseHandle(handle);
            result
        }
    }

    /// Terminates the process tree "from the inside out" and waits for the root
    /// to exit naturally.
    ///
    /// 1. Terminate every Job member except the root.
    /// 2. Wait up to `grace` for the root to exit naturally
    ///    (`WaitForSingleObject`, no polling).
    /// 3. Only if the grace period is exceeded, force-kill the root.
    ///
    /// If the root has no descendants, it is terminated immediately without
    /// waiting (same as a regular process). A root that has already exited is
    /// treated as success.
    pub async fn shutdown_tree(
        &self,
        root_pid: u32,
        grace: std::time::Duration,
    ) -> Result<TreeShutdownOutcome, PtyError> {
        // Whether the launcher is wrapper-style is decided by the member
        // composition, not by the success of the termination request (so that the
        // cleanup wait is not skipped even if an inner process exited naturally
        // just before).
        let has_descendants = self
            .job
            .process_ids()?
            .into_iter()
            .any(|pid| pid != 0 && pid != root_pid);
        let terminated_descendants = self.job.terminate_all_except(root_pid)?;
        if !has_descendants {
            terminate_process(root_pid);
            return Ok(TreeShutdownOutcome {
                terminated_descendants,
                root_exited_naturally: false,
            });
        }
        // Wait for the root (wrapper) to finish its cleanup and exit on its own.
        let timeout_ms = u32::try_from(grace.as_millis()).unwrap_or(u32::MAX);
        let exited =
            tokio::task::spawn_blocking(move || wait_for_process_exit(root_pid, timeout_ms))
                .await
                .unwrap_or(false);
        if !exited {
            terminate_process(root_pid);
        }
        Ok(TreeShutdownOutcome {
            terminated_descendants,
            root_exited_naturally: exited,
        })
    }
}

/// Process tree shutdown policy (Unix implementation, with known limitations).
///
/// There is no equivalent of a Job Object, so processes other than the root
/// cannot be enumerated or terminated. Only the root (the direct child) is
/// `SIGKILL`ed, and cleanup waiting is left to the caller's `Child::status()`.
///
/// As a result, processes inside wrapper launchers may be orphaned (the official
/// ACP SDK handles this by killing the process group, but that terminates the
/// root as well, so the wrapper's cleanup does not run). Details:
/// `docs/proposals/2026-10-01-antigravity-startup.md`.
#[cfg(not(windows))]
impl ProcessTreeGuard {
    /// Does nothing on Unix, which has no Job Object equivalent (API
    /// compatibility only).
    pub fn attach_pid(&self, pid: u32) -> Result<(), PtyError> {
        let _ = pid;
        Ok(())
    }

    /// Terminates the process tree (root `SIGKILL` only).
    ///
    /// Unlike the Windows implementation, does not wait or enumerate descendants.
    /// `grace` is accepted for API compatibility but unused.
    pub async fn shutdown_tree(
        &self,
        root_pid: u32,
        grace: std::time::Duration,
    ) -> Result<TreeShutdownOutcome, PtyError> {
        // Why no waiting: reaping the root (`waitpid`) would race with the
        // caller's `Child::status()`, so it is not done on the fxg-pty side.
        let _ = grace;
        unsafe {
            if libc::kill(root_pid as libc::pid_t, libc::SIGKILL) != 0 {
                let err = std::io::Error::last_os_error();
                // ESRCH (already exited) is a normal case
                if err.raw_os_error() != Some(libc::ESRCH) {
                    tracing::debug!(root_pid, "failed to kill process: {err}");
                }
            }
        }
        Ok(TreeShutdownOutcome {
            terminated_descendants: 0,
            root_exited_naturally: false,
        })
    }
}

/// Thin wrapper around a Windows Job Object (`kill on job close`).
///
/// Dropping closes the Job handle and terminates all assigned processes
/// (including grandchildren). This is mandatory machinery so that agent child
/// processes cannot leave file locks behind even if they slip past
/// `Child::kill()` (design: `docs/04-agent-drivers-and-windows.md` §5.1).
#[cfg(windows)]
pub struct WinJobGuard {
    job: win32job::Job,
}

#[cfg(windows)]
impl WinJobGuard {
    /// Creates a Job Object with kill-on-close enabled.
    pub fn new_kill_on_close() -> Result<Self, PtyError> {
        let job = win32job::Job::create()?;
        let mut info = job.query_extended_limit_info()?;
        info.limit_kill_on_job_close();
        job.set_extended_limit_info(&info)?;
        Ok(Self { job })
    }

    /// Assigns a process handle to the Job Object.
    pub fn assign_process(
        &self,
        process_handle: std::os::windows::io::RawHandle,
    ) -> Result<(), PtyError> {
        self.job.assign_process(process_handle as isize)?;
        Ok(())
    }

    /// Assigns the process holding the guard (the daemon itself) to the Job
    /// Object.
    pub fn assign_current_process(&self) -> Result<(), PtyError> {
        self.job.assign_current_process()?;
        Ok(())
    }

    /// Returns the list of process IDs currently assigned to the Job Object.
    ///
    /// If the number of Job members exceeds a practically impossible limit
    /// (`MAX_JOB_MEMBERS`), gives up and returns an error (the caller falls back
    /// to the previous shutdown path).
    pub fn process_ids(&self) -> Result<Vec<u32>, PtyError> {
        /// Maximum number of Job members retrieved at once (agent trees are
        /// usually under 10).
        const MAX_JOB_MEMBERS: usize = 1024;
        /// Byte offset to `JOBOBJECT_BASIC_PROCESS_ID_LIST.ProcessIdList`.
        const HEADER_BYTES: usize =
            std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList);

        // `JOBOBJECT_BASIC_PROCESS_ID_LIST` is variable-length (ULONG_PTR array),
        // so allocate in `usize` units to guarantee alignment.
        let mut buffer = vec![
            0usize;
            (HEADER_BYTES + MAX_JOB_MEMBERS * std::mem::size_of::<usize>())
                .div_ceil(std::mem::size_of::<usize>())
        ];
        let buffer_bytes =
            u32::try_from(buffer.len() * std::mem::size_of::<usize>()).unwrap_or(u32::MAX);
        unsafe {
            QueryInformationJobObject(
                Some(HANDLE(self.job.handle() as *mut core::ffi::c_void)),
                JobObjectBasicProcessIdList,
                buffer.as_mut_ptr().cast(),
                buffer_bytes,
                None,
            )?;
            let count = (*buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>())
                .NumberOfProcessIdsInList as usize;
            // The head of the ULONG_PTR array is the slot at index
            // `HEADER_BYTES / size_of::<usize>()`.
            let list = buffer
                .as_ptr()
                .add(HEADER_BYTES / std::mem::size_of::<usize>());
            Ok((0..count.min(MAX_JOB_MEMBERS))
                .map(|index| *list.add(index) as u32)
                .collect())
        }
    }

    /// Terminates every Job member except the given PID (best effort).
    ///
    /// Returns the number of processes terminated (already-exited ones are not
    /// counted).
    pub fn terminate_all_except(&self, keep_pid: u32) -> Result<usize, PtyError> {
        let mut terminated = 0usize;
        for pid in self.process_ids()? {
            if pid == 0 || pid == keep_pid {
                continue;
            }
            if terminate_process(pid) {
                terminated += 1;
            }
        }
        Ok(terminated)
    }
}

/// Force-terminates a process (best effort; does nothing if it already exited).
///
/// Returns whether `TerminateProcess` succeeded.
#[cfg(windows)]
fn terminate_process(pid: u32) -> bool {
    unsafe {
        match OpenProcess(PROCESS_TERMINATE, false, pid) {
            Ok(handle) => {
                let terminated = TerminateProcess(handle, 1).is_ok();
                let _ = CloseHandle(handle);
                terminated
            }
            Err(err) => {
                tracing::debug!(pid, "failed to open process for termination: {err}");
                false
            }
        }
    }
}

/// Waits up to `timeout_ms` milliseconds for the process to exit (no polling).
///
/// Returns `true` if the process no longer exists. Waits on the process handle's
/// signal state via `WaitForSingleObject`.
#[cfg(windows)]
fn wait_for_process_exit(pid: u32, timeout_ms: u32) -> bool {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else {
            // The process has already exited
            return true;
        };
        let result = WaitForSingleObject(handle, timeout_ms);
        let _ = CloseHandle(handle);
        result == WAIT_OBJECT_0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_command_finds_shell_tools() {
        let cwd = std::env::current_dir().expect("cwd");
        let resolved = resolve_command("git", &cwd).expect("git must be resolvable");
        assert!(resolved.is_absolute());
    }

    #[test]
    fn resolve_command_reports_missing_command() {
        let cwd = std::env::current_dir().expect("cwd");
        let err = resolve_command("fxg-definitely-missing-binary", &cwd).expect_err("missing");
        match err {
            PtyError::CommandNotFound { command, .. } => {
                assert_eq!(command, "fxg-definitely-missing-binary");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn canonicalize_avoids_windows_unc_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolved = canonicalize(dir.path()).expect("canonicalize");
        assert!(
            !resolved.to_string_lossy().starts_with(r"\\?\"),
            "dunce must not add a UNC prefix: {}",
            resolved.display()
        );
    }

    #[test]
    fn process_tree_guard_can_be_created() {
        let guard = ProcessTreeGuard::new().expect("guard");
        assert_eq!(guard.is_job_object_backend(), cfg!(windows));
    }

    /// Windows Job Object: a **grandchild** spawned by the root process must be
    /// reliably terminated by kill-on-close when the guard (Job) is dropped
    /// (Phase 2 completion criterion / CI: automatically verified on
    /// windows-latest).
    #[cfg(windows)]
    #[test]
    fn job_object_kills_grandchildren_on_guard_drop() {
        use std::os::windows::io::AsRawHandle;
        use std::process::Command as StdCommand;

        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("grandchild.pid");
        let ping_out = dir.path().join("grandchild.out");
        let ping_err = dir.path().join("grandchild.err");

        let guard = ProcessTreeGuard::new().expect("guard");

        // Assign the root (PowerShell) to the Job first, then let it spawn the
        // grandchild (ping). Processes created by a Job member automatically
        // belong to the same Job, so the grandchild stays in the Job even after
        // the root exits. The grace period (Start-Sleep) ensures the root's Job
        // assignment completes before the grandchild is spawned.
        //
        // `-NoNewWindow` + output redirection avoids **creating a new console**:
        // the default `Start-Process` would create a new console window for the
        // grandchild, popping up a ping window during the test run, and if the
        // Job's kill-on-close races with console initialization, conhost writes
        // "error 0x800700e8 at startup (ERROR_NO_DATA: the pipe is being closed)"
        // to that window (the test itself passes, but it is noise).
        let script = format!(
            "Start-Sleep -Milliseconds 1500; \
             $p = Start-Process -PassThru -NoNewWindow -FilePath ping \
                  -ArgumentList '-n','30','127.0.0.1' \
                  -RedirectStandardOutput '{}' -RedirectStandardError '{}'; \
             Set-Content -Path '{}' -Value $p.Id",
            ping_out.display(),
            ping_err.display(),
            pid_file.display()
        );
        let mut root = StdCommand::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn root");
        guard
            .job
            .assign_process(root.as_raw_handle())
            .expect("assign root process to job");

        let grandchild_pid = wait_for_pid_file(&pid_file);
        assert!(
            windows_process_is_running(grandchild_pid),
            "grandchild ({grandchild_pid}) must be running before guard drop"
        );
        let _ = root.wait();

        // Guard drop → Job handle close → kill-on-close terminates the grandchild
        drop(guard);

        let mut killed = false;
        for _ in 0..100 {
            if !windows_process_is_running(grandchild_pid) {
                killed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !killed {
            // Do not leave ping behind if the test fails (best effort)
            let _ = StdCommand::new("taskkill")
                .args(["/PID", &grandchild_pid.to_string(), "/T", "/F"])
                .status();
        }
        assert!(killed, "grandchild process survived job object close");
    }

    /// Windows Job Object: [`ProcessTreeGuard::terminate_all_except`] must
    /// terminate every member except the root (verifies shutdown behavior that
    /// does not prevent the automatic temp-file cleanup of PyInstaller onefile
    /// and similar).
    #[cfg(windows)]
    #[test]
    fn terminate_all_except_spares_root_but_kills_the_rest() {
        use std::os::windows::io::AsRawHandle;
        use std::process::Command as StdCommand;

        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("child.pid");
        let ping_out = dir.path().join("child.out");
        let ping_err = dir.path().join("child.err");

        let guard = ProcessTreeGuard::new().expect("guard");

        // The root (powershell) spawns the grandchild (ping) and then sleeps for
        // 30 seconds
        let script = format!(
            "$p = Start-Process -PassThru -NoNewWindow -FilePath ping \
                  -ArgumentList '-n','30','127.0.0.1' \
                  -RedirectStandardOutput '{}' -RedirectStandardError '{}'; \
             Set-Content -Path '{}' -Value $p.Id; \
             Start-Sleep -Seconds 30",
            ping_out.display(),
            ping_err.display(),
            pid_file.display()
        );
        let mut root = StdCommand::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn root");
        guard
            .job
            .assign_process(root.as_raw_handle())
            .expect("assign root process to job");

        let child_pid = wait_for_pid_file(&pid_file);
        assert!(
            windows_process_is_running(child_pid),
            "child ({child_pid}) must be running before termination"
        );

        // Terminate everything except the root (the grandchild)
        let terminated = guard.terminate_all_except(root.id()).expect("terminate");
        assert!(terminated >= 1, "expected at least one terminated member");

        let mut killed = false;
        for _ in 0..100 {
            if !windows_process_is_running(child_pid) {
                killed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !killed {
            // Do not leave ping behind if the test fails (best effort)
            let _ = StdCommand::new("taskkill")
                .args(["/PID", &child_pid.to_string(), "/T", "/F"])
                .status();
        }
        assert!(killed, "child process survived terminate_all_except");
        assert!(
            windows_process_is_running(root.id()),
            "root process must be spared by terminate_all_except"
        );

        // Cleanup (the guard drop's kill-on-close also terminates the root)
        drop(guard);
        let _ = root.kill();
        let _ = root.wait();
    }

    /// Windows Job Object: [`ProcessTreeGuard::shutdown_tree`] must wait for a
    /// wrapper launcher to finish its cleanup (natural exit). The root detects
    /// the grandchild's exit, writes a marker file, and then exits on its own.
    #[cfg(windows)]
    #[tokio::test]
    async fn shutdown_tree_waits_for_wrapper_cleanup() {
        use std::os::windows::io::AsRawHandle;
        use std::process::Command as StdCommand;

        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("child.pid");
        let cleanup_marker = dir.path().join("cleanup.done");
        let ping_out = dir.path().join("child.out");
        let ping_err = dir.path().join("child.err");

        let guard = ProcessTreeGuard::new().expect("guard");

        // The root (powershell) spawns the grandchild (ping), waits for it to
        // exit, then performs its own cleanup (marker file) and exits,
        // simulating a wrapper.
        let script = format!(
            "$p = Start-Process -PassThru -NoNewWindow -FilePath ping \
                  -ArgumentList '-n','60','127.0.0.1' \
                  -RedirectStandardOutput '{}' -RedirectStandardError '{}'; \
             Set-Content -Path '{}' -Value $p.Id; \
             Wait-Process -Id $p.Id; \
             Set-Content -Path '{}' -Value 'cleaned'",
            ping_out.display(),
            ping_err.display(),
            pid_file.display(),
            cleanup_marker.display()
        );
        let mut root = StdCommand::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn root");
        guard
            .job
            .assign_process(root.as_raw_handle())
            .expect("assign root process to job");

        let child_pid = wait_for_pid_file(&pid_file);
        assert!(
            windows_process_is_running(child_pid),
            "child must be running"
        );

        let outcome = guard
            .shutdown_tree(root.id(), std::time::Duration::from_secs(15))
            .await
            .expect("shutdown_tree");
        assert!(
            outcome.terminated_descendants >= 1,
            "wrapper child must be terminated"
        );
        assert!(
            outcome.root_exited_naturally,
            "wrapper must exit naturally after its child exits"
        );
        assert!(
            cleanup_marker.is_file(),
            "wrapper cleanup must run before exit"
        );
        assert!(!windows_process_is_running(root.id()));
        let _ = root.wait();
    }

    /// Waits until the grandchild's PID is written to the file.
    #[cfg(windows)]
    fn wait_for_pid_file(path: &Path) -> u32 {
        for _ in 0..150 {
            if let Ok(text) = std::fs::read_to_string(path)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                return pid;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("grandchild pid file was not written: {}", path.display());
    }

    /// Checks with `tasklist` whether the process with the given PID is running.
    #[cfg(windows)]
    fn windows_process_is_running(pid: u32) -> bool {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .expect("tasklist");
        let text = String::from_utf8_lossy(&output.stdout);
        text.split_whitespace()
            .any(|field| field == pid.to_string())
    }
}
