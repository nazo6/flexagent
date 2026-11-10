//! `fxg service` — register the OS login autostart service.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §5.4,
//! `docs/05-cli-and-pwa-ui.md` §1.
//!
//! - **Windows**: Task Scheduler (runs at logon; no console window is shown)
//! - **Linux / WSL**: `~/.config/systemd/user/<name>.service` + `systemctl --user`
//! - **macOS**: `~/Library/LaunchAgents/dev.flexagent.<name>.plist` + `launchctl`
//!
//! `service` only installs / uninstalls the registration. Runtime start / stop /
//! restart is delegated from `fxg node ...` / `fxg hub ...` when a unit exists.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Process managed by an OS service unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceTarget {
    /// Local node (`fxg node start`)
    Node,
    /// Hub (`fxg hub start`)
    Hub,
}

impl ServiceTarget {
    /// Service / unit name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Node => "fxg-node",
            Self::Hub => "fxg-hub",
        }
    }

    /// Command the unit runs.
    pub const fn command(self) -> &'static [&'static str] {
        match self {
            Self::Node => &["node", "start"],
            Self::Hub => &["hub", "start"],
        }
    }

    /// Log file name.
    pub const fn log_name(self) -> &'static str {
        match self {
            Self::Node => "node.log",
            Self::Hub => "hub.log",
        }
    }
}

/// Service action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    /// Register the service (enable auto-start)
    Install,
    /// Remove the service registration
    Uninstall,
}

/// Run `fxg service <action> [node|hub]`.
pub async fn run(action: ServiceAction, target: ServiceTarget) -> Result<()> {
    run_sync(action, target)
}

/// Synchronous core of `fxg service <action> [node|hub]`.
pub fn run_sync(action: ServiceAction, target: ServiceTarget) -> Result<()> {
    let exe = std::env::current_exe().context("failed to resolve the fxg executable path")?;
    platform_run(action, target, target.name(), &exe)
}

/// Whether the service is registered (for `node status` / `hub status` and the tray).
#[cfg(windows)]
pub fn is_installed(target: ServiceTarget) -> bool {
    let name = target.name();
    task_exists(name)
        || startup_script_path(name)
            .map(|path| path.exists())
            .unwrap_or(false)
}

/// Whether the service is registered (whether the systemd unit file exists).
#[cfg(target_os = "linux")]
pub fn is_installed(target: ServiceTarget) -> bool {
    unit_path(target.name())
        .map(|path| path.exists())
        .unwrap_or(false)
}

/// Whether the service is registered (whether the LaunchAgent plist exists).
#[cfg(target_os = "macos")]
pub fn is_installed(target: ServiceTarget) -> bool {
    plist_path(target.name())
        .map(|path| path.exists())
        .unwrap_or(false)
}

/// Starts the registered service (delegated from `fxg node start` / `fxg hub start`).
pub fn start(target: ServiceTarget) -> Result<()> {
    platform_start(target)
}

/// Stops the registered service (delegated from `fxg node stop` / `fxg hub stop`).
pub fn stop(target: ServiceTarget) -> Result<()> {
    platform_stop(target)
}

/// Restarts the registered service.
pub fn restart(target: ServiceTarget) -> Result<()> {
    platform_restart(target)
}

// ----------------------------------------------------------------------
// Windows: Task Scheduler
// ----------------------------------------------------------------------

#[cfg(windows)]
fn platform_run(
    action: ServiceAction,
    target: ServiceTarget,
    name: &str,
    exe: &std::path::Path,
) -> Result<()> {
    let args = target.command().join(" ");
    let task_run = format!("\"\\\"{}\\\" {args}\"", exe.display());
    let startup_script = startup_script_path(name)?;

    match action {
        ServiceAction::Install => {
            match schtasks(&[
                "/Create", "/F", "/SC", "ONLOGON", "/TN", name, "/TR", &task_run,
            ]) {
                Ok(()) => {
                    println!("service installed (Task Scheduler, at logon): {name}");
                }
                Err(err) => {
                    eprintln!("failed to register the Task Scheduler task: {err}");
                    write_startup_script(&startup_script, exe, target)?;
                    println!(
                        "service installed (Startup folder): {}",
                        startup_script.display()
                    );
                }
            }
        }
        ServiceAction::Uninstall => {
            if let Err(err) = schtasks(&["/End", "/TN", name]) {
                tracing::warn!("failed to end scheduled task {name}: {err:#}");
            }
            let task_removed = schtasks(&["/Delete", "/F", "/TN", name]).is_ok();
            let script_removed = if startup_script.exists() {
                std::fs::remove_file(&startup_script)?;
                true
            } else {
                false
            };
            if !task_removed && !script_removed {
                bail!("no service registration found: {name}");
            }
            println!("service uninstalled: {name}");
        }
    }
    Ok(())
}

#[cfg(windows)]
fn platform_start(target: ServiceTarget) -> Result<()> {
    let name = target.name();
    if task_exists(name) {
        schtasks(&["/Run", "/TN", name])?;
    } else {
        let startup_script = startup_script_path(name)?;
        if startup_script.exists() {
            start_via_script(&startup_script)?;
        } else {
            bail!("service is not installed; run `fxg service install {name}`");
        }
    }
    Ok(())
}

#[cfg(windows)]
fn platform_stop(target: ServiceTarget) -> Result<()> {
    let name = target.name();
    if task_exists(name) {
        schtasks(&["/End", "/TN", name])?;
    } else {
        let exe = std::env::current_exe().context("failed to resolve the fxg executable path")?;
        stop_by_command_line(&exe, target.command())?;
    }
    Ok(())
}

#[cfg(windows)]
fn platform_restart(target: ServiceTarget) -> Result<()> {
    let name = target.name();
    if task_exists(name) {
        if let Err(err) = schtasks(&["/End", "/TN", name]) {
            tracing::warn!("failed to stop scheduled task {name}: {err:#}");
        }
        schtasks(&["/Run", "/TN", name])?;
    } else {
        platform_stop(target)?;
        platform_start(target)?;
    }
    Ok(())
}

#[cfg(windows)]
fn schtasks(args: &[&str]) -> Result<()> {
    let output = Command::new("schtasks")
        .args(args)
        .output()
        .context("failed to run schtasks")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "schtasks {} failed: {}{}",
            args.join(" "),
            stdout.trim(),
            stderr.trim()
        );
    }
    Ok(())
}

#[cfg(windows)]
fn task_exists(name: &str) -> bool {
    Command::new("schtasks")
        .args(["/Query", "/TN", name])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// VBS launcher path in the Startup folder (`%APPDATA%\...\Startup\<name>.vbs`).
#[cfg(windows)]
fn startup_script_path(name: &str) -> Result<PathBuf> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .context("APPDATA is not set")?;
    Ok(appdata
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join(format!("{name}.vbs")))
}

/// Write a VBS launcher that starts the target without showing a console window.
#[cfg(windows)]
fn write_startup_script(
    path: &std::path::Path,
    exe: &std::path::Path,
    target: ServiceTarget,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let args = target.command().join(" ");
    // Second argument 0 = hide the window (equivalent to CREATE_NO_WINDOW)
    let body = format!(
        "' FlexAgent auto-start launcher (generated by `fxg service install`)\r\n\
         CreateObject(\"WScript.Shell\").Run \"\"\"{exe}\"\" {args}\", 0, False\r\n",
        exe = exe.display()
    );
    std::fs::write(path, body)?;
    Ok(())
}

#[cfg(windows)]
fn start_via_script(path: &std::path::Path) -> Result<()> {
    Command::new("wscript.exe")
        .arg(path)
        .spawn()
        .context("failed to run the startup launcher (wscript.exe)")?;
    Ok(())
}

/// Stop the running fxg process (matching arguments) through PowerShell CIM.
#[cfg(windows)]
fn stop_by_command_line(exe: &std::path::Path, args: &[&str]) -> Result<()> {
    let exe_name = exe
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "fxg.exe".to_owned());
    let needle = args.join(" ");
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='{exe_name}'\" | \
         Where-Object {{ $_.CommandLine -and $_.CommandLine.Contains('{needle}') }} | \
         ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .context("failed to run powershell for process termination")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("failed to stop the service process: {}", stderr.trim());
    }
    Ok(())
}

// ----------------------------------------------------------------------
// Linux / WSL: systemd --user
// ----------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn unit_path(name: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("failed to resolve the home directory")?;
    Ok(home
        .join(".config/systemd/user")
        .join(format!("{name}.service")))
}

#[cfg(target_os = "linux")]
fn platform_run(
    action: ServiceAction,
    target: ServiceTarget,
    name: &str,
    exe: &std::path::Path,
) -> Result<()> {
    let unit = unit_path(name)?;
    match action {
        ServiceAction::Install => {
            if let Some(parent) = unit.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let args = target.command().join(" ");
            let description = match target {
                ServiceTarget::Node => "FlexAgent node",
                ServiceTarget::Hub => "FlexAgent hub",
            };
            let unit_body = format!(
                "[Unit]\n\
                 Description={description}\n\
                 After=network-online.target\n\
                 \n\
                 [Service]\n\
                 ExecStart={} {args}\n\
                 Restart=on-failure\n\
                 RestartSec=3\n\
                 \n\
                 [Install]\n\
                 WantedBy=default.target\n",
                exe.display()
            );
            std::fs::write(&unit, unit_body)?;
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", "--now", name])?;
            println!("service installed: {} ({})", name, unit.display());
        }
        ServiceAction::Uninstall => {
            if let Err(err) = systemctl(&["disable", "--now", name]) {
                tracing::warn!("failed to disable service {name}: {err:#}");
            }
            if unit.exists() {
                std::fs::remove_file(&unit)?;
            }
            if let Err(err) = systemctl(&["daemon-reload"]) {
                tracing::warn!("failed to reload systemd after uninstall: {err:#}");
            }
            println!("service uninstalled: {name}");
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn platform_start(target: ServiceTarget) -> Result<()> {
    systemctl(&["start", target.name()])
}

#[cfg(target_os = "linux")]
fn platform_stop(target: ServiceTarget) -> Result<()> {
    systemctl(&["stop", target.name()])
}

#[cfg(target_os = "linux")]
fn platform_restart(target: ServiceTarget) -> Result<()> {
    systemctl(&["restart", target.name()])
}

#[cfg(target_os = "linux")]
fn systemctl(args: &[&str]) -> Result<()> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("failed to run systemctl (systemd --user is required)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "systemctl --user {} failed: {}",
            args.join(" "),
            stderr.trim()
        );
    }
    Ok(())
}

// ----------------------------------------------------------------------
// macOS: launchd
// ----------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn plist_label(name: &str) -> String {
    format!("dev.flexagent.{name}")
}

#[cfg(target_os = "macos")]
fn plist_path(name: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("failed to resolve the home directory")?;
    Ok(home
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", plist_label(name))))
}

#[cfg(target_os = "macos")]
fn platform_run(
    action: ServiceAction,
    target: ServiceTarget,
    name: &str,
    exe: &std::path::Path,
) -> Result<()> {
    let plist = plist_path(name)?;
    let label = plist_label(name);
    match action {
        ServiceAction::Install => {
            if let Some(parent) = plist.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let args = target
                .command()
                .iter()
                .map(|arg| format!("    <string>{arg}</string>"))
                .collect::<Vec<_>>()
                .join("\n");
            let body = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
                 \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                 <plist version=\"1.0\">\n\
                 <dict>\n\
                 \x20 <key>Label</key>\n\
                 \x20 <string>{label}</string>\n\
                 \x20 <key>ProgramArguments</key>\n\
                 \x20 <array>\n\
                 \x20   <string>{exe}</string>\n\
                 {args}\n\
                 \x20 </array>\n\
                 \x20 <key>RunAtLoad</key><true/>\n\
                 \x20 <key>KeepAlive</key><true/>\n\
                 </dict>\n\
                 </plist>\n",
                exe = exe.display()
            );
            std::fs::write(&plist, body)?;
            launchctl(&["load", "-w", &plist.display().to_string()])?;
            println!("service installed: {} ({})", label, plist.display());
        }
        ServiceAction::Uninstall => {
            if let Err(err) = launchctl(&["unload", "-w", &plist.display().to_string()]) {
                tracing::warn!("failed to unload service {label}: {err:#}");
            }
            if plist.exists() {
                std::fs::remove_file(&plist)?;
            }
            println!("service uninstalled: {label}");
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn platform_start(target: ServiceTarget) -> Result<()> {
    let plist = plist_path(target.name())?;
    launchctl(&["load", "-w", &plist.display().to_string()])
}

#[cfg(target_os = "macos")]
fn platform_stop(target: ServiceTarget) -> Result<()> {
    let label = plist_label(target.name());
    let target_uid = format!("gui/{}", uid());
    launchctl(&["kill", "SIGTERM", &format!("{target_uid}/{label}")])
}

#[cfg(target_os = "macos")]
fn platform_restart(target: ServiceTarget) -> Result<()> {
    let label = plist_label(target.name());
    let target_uid = format!("gui/{}", uid());
    launchctl(&["kickstart", "-k", &format!("{target_uid}/{label}")])
}

#[cfg(target_os = "macos")]
fn uid() -> u32 {
    // SAFETY: `getuid` always succeeds
    unsafe { libc::getuid() }
}

#[cfg(target_os = "macos")]
fn launchctl(args: &[&str]) -> Result<()> {
    let output = Command::new("launchctl")
        .args(args)
        .output()
        .context("failed to run launchctl")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("launchctl {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(())
}
