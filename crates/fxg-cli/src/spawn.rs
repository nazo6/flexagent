//! Detached process spawning for `fxg node start --detach` / `fxg hub start --detach`.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// Spawns `fxg <args...>` detached with its stdio redirected to
/// `~/.flexagent/logs/<log_name>` and returns the log path.
pub fn spawn_background(args: &[&str], log_name: &str) -> Result<PathBuf> {
    let env = fxg_protocol::config::process_env;
    let home = fxg_protocol::config::fxg_home(&env);
    let log_path = home.join("logs").join(log_name);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;
    let stderr = log
        .try_clone()
        .with_context(|| format!("failed to clone {}", log_path.display()))?;

    let exe = std::env::current_exe().context("failed to resolve the fxg executable")?;
    let mut command = Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // New session so the process survives terminal close (SIGHUP).
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, DETACHED_PROCESS};
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }

    let child = command
        .spawn()
        .with_context(|| format!("failed to spawn `fxg {}`", args.join(" ")))?;
    // Detached: the process is reparented (Unix) / detached (Windows).
    drop(child);
    Ok(log_path)
}
