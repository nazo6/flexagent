//! `fxg service` — manage the background service that starts at OS login.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §5.4,
//! `docs/05-cli-and-pwa-ui.md` §1.
//!
//! - **Windows**: Task Scheduler (runs at logon; no console window is shown)
//! - **Linux / WSL**: `~/.config/systemd/user/<name>.service` + `systemctl --user`
//! - **macOS**: `~/Library/LaunchAgents/dev.flexagent.<name>.plist` + `launchctl`
//!
//! With `--server`, the central server is installed as the service instead of
//! the daemon.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Service action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    /// Register the service (enable auto-start)
    Install,
    /// Remove the service registration
    Uninstall,
    /// Start the service
    Start,
    /// Stop the service
    Stop,
    /// Restart the service
    Restart,
    /// Show the service status
    Status,
}

impl ServiceAction {
    /// Parse the CLI `<action>` string.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "install" => Some(Self::Install),
            "uninstall" => Some(Self::Uninstall),
            "start" => Some(Self::Start),
            "stop" => Some(Self::Stop),
            "restart" => Some(Self::Restart),
            "status" => Some(Self::Status),
            _ => None,
        }
    }
}

/// Service name (`fxg-daemon` / `fxg-server`).
fn service_name(server: bool) -> &'static str {
    if server { "fxg-server" } else { "fxg-daemon" }
}

/// Subcommand arguments the service runs.
fn service_args(server: bool) -> &'static [&'static str] {
    if server { &["server"] } else { &["daemon"] }
}

/// Run `fxg service <action>`.
pub async fn run(action: ServiceAction, server: bool) -> Result<()> {
    if action == ServiceAction::Status {
        let exe = std::env::current_exe().context("failed to resolve the fxg executable path")?;
        return status(server, &exe).await;
    }
    run_sync(action, server)
}

/// Synchronous core of `fxg service <action>` (everything except `status`; no
/// CLI output).
///
/// The "auto-start" toggle in the tray menu reuses the same install/uninstall
/// logic.
pub fn run_sync(action: ServiceAction, server: bool) -> Result<()> {
    debug_assert!(
        action != ServiceAction::Status,
        "status is handled by `run`"
    );
    let name = service_name(server);
    let exe = std::env::current_exe().context("failed to resolve the fxg executable path")?;
    platform_run(action, server, name, &exe)
}

/// Whether auto-start is registered (for the tray check mark; no side effects).
#[cfg(windows)]
pub fn is_installed(server: bool) -> bool {
    let name = service_name(server);
    task_exists(name)
        || startup_script_path(name)
            .map(|path| path.exists())
            .unwrap_or(false)
}

/// Whether auto-start is registered (whether the systemd unit file exists).
#[cfg(target_os = "linux")]
pub fn is_installed(server: bool) -> bool {
    unit_path(service_name(server))
        .map(|path| path.exists())
        .unwrap_or(false)
}

/// Whether auto-start is registered (whether the LaunchAgent plist exists).
#[cfg(target_os = "macos")]
pub fn is_installed(server: bool) -> bool {
    plist_path(service_name(server))
        .map(|path| path.exists())
        .unwrap_or(false)
}

// ----------------------------------------------------------------------
// Windows: Task Scheduler
// ----------------------------------------------------------------------

#[cfg(windows)]
fn platform_run(
    action: ServiceAction,
    server: bool,
    name: &str,
    exe: &std::path::Path,
) -> Result<()> {
    // Build `/TR "\"<exe>\" <sub>"` (schtasks accepts quoted paths with spaces)
    let args = service_args(server).join(" ");
    let task_run = format!("\"\\\"{}\\\" {args}\"", exe.display());
    let startup_script = startup_script_path(name)?;

    match action {
        ServiceAction::Install => {
            // Try Task Scheduler first. On machines without administrator
            // rights, fall back to the Startup folder (VBS launcher).
            match schtasks(&[
                "/Create", "/F", "/SC", "ONLOGON", "/TN", name, "/TR", &task_run,
            ]) {
                Ok(()) => {
                    println!("service installed (Task Scheduler, at logon): {name}");
                }
                Err(err) => {
                    eprintln!("failed to register the Task Scheduler task: {err}");
                    write_startup_script(&startup_script, exe, server)?;
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
        ServiceAction::Start => {
            if task_exists(name) {
                schtasks(&["/Run", "/TN", name])?;
            } else if startup_script.exists() {
                start_via_script(&startup_script)?;
            } else {
                bail!("service is not installed; run `fxg service install`");
            }
            println!("service started: {name}");
        }
        ServiceAction::Stop => {
            if task_exists(name) {
                schtasks(&["/End", "/TN", name])?;
            } else {
                stop_by_command_line(exe, service_args(server))?;
            }
            println!("service stopped: {name}");
        }
        ServiceAction::Restart => {
            if task_exists(name) {
                if let Err(err) = schtasks(&["/End", "/TN", name]) {
                    tracing::warn!("failed to stop scheduled task {name}: {err:#}");
                }
                schtasks(&["/Run", "/TN", name])?;
            } else if startup_script.exists() {
                if let Err(err) = stop_by_command_line(exe, service_args(server)) {
                    tracing::warn!("failed to stop service {name}: {err:#}");
                }
                start_via_script(&startup_script)?;
            } else {
                bail!("service is not installed; run `fxg service install`");
            }
            println!("service restarted: {name}");
        }
        ServiceAction::Status => unreachable!("status is handled by the caller"),
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
fn write_startup_script(path: &std::path::Path, exe: &std::path::Path, server: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let args = service_args(server).join(" ");
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

#[cfg(windows)]
async fn status(server: bool, _exe: &std::path::Path) -> Result<()> {
    let name = service_name(server);
    let task = task_exists(name);
    let startup = startup_script_path(name)?.exists();
    let installed = task || startup;
    let running = probe_running(server).await;
    println!(
        "service:   {name} ({})",
        if task {
            "windows task scheduler"
        } else if startup {
            "startup folder (VBS launcher)"
        } else {
            "not registered"
        }
    );
    println!("installed: {}", yes_no(installed));
    println!("running:   {}", yes_no(running));
    println!("log file:  {}", service_log_path(server).display());
    if !installed {
        println!(
            "hint:      register it with `fxg service install{}`",
            server_flag(server)
        );
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
    server: bool,
    name: &str,
    exe: &std::path::Path,
) -> Result<()> {
    let unit = unit_path(name)?;
    match action {
        ServiceAction::Install => {
            if let Some(parent) = unit.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let args = service_args(server).join(" ");
            let description = if server {
                "FlexAgent central server"
            } else {
                "FlexAgent node daemon"
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
        ServiceAction::Start => {
            systemctl(&["start", name])?;
            println!("service started: {name}");
        }
        ServiceAction::Stop => {
            systemctl(&["stop", name])?;
            println!("service stopped: {name}");
        }
        ServiceAction::Restart => {
            systemctl(&["restart", name])?;
            println!("service restarted: {name}");
        }
        ServiceAction::Status => unreachable!("status is handled by the caller"),
    }
    Ok(())
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

#[cfg(target_os = "linux")]
async fn status(server: bool, _exe: &std::path::Path) -> Result<()> {
    let name = service_name(server);
    let unit = unit_path(name)?;
    let active = Command::new("systemctl")
        .args(["--user", "is-active", name])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|_| "unknown".to_owned());
    let enabled = Command::new("systemctl")
        .args(["--user", "is-enabled", name])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|_| "unknown".to_owned());
    let installed = enabled == "enabled" || unit.exists();
    println!("service:   {name} (systemd --user)");
    println!("unit:      {}", unit.display());
    println!("installed: {}", yes_no(installed));
    println!("running:   {}", active);
    println!("probe:     {}", yes_no(probe_running(server).await));
    println!("log file:  {}", service_log_path(server).display());
    if !installed {
        println!(
            "hint:      register it with `fxg service install{}`",
            server_flag(server)
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
    server: bool,
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
            let args = service_args(server)
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
        ServiceAction::Start => {
            launchctl(&["load", "-w", &plist.display().to_string()])?;
            println!("service started: {label}");
        }
        ServiceAction::Stop => {
            let target = format!("gui/{}", uid());
            launchctl(&["kill", "SIGTERM", &format!("{target}/{label}")])?;
            println!("service stopped: {label}");
        }
        ServiceAction::Restart => {
            let target = format!("gui/{}", uid());
            launchctl(&["kickstart", "-k", &format!("{target}/{label}")])?;
            println!("service restarted: {label}");
        }
        ServiceAction::Status => unreachable!("status is handled by the caller"),
    }
    Ok(())
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

#[cfg(target_os = "macos")]
async fn status(server: bool, _exe: &std::path::Path) -> Result<()> {
    let name = service_name(server);
    let label = plist_label(name);
    let plist = plist_path(name)?;
    let installed = plist.exists();
    let running = Command::new("launchctl")
        .args(["list", &label])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    println!("service:   {label} (launchd)");
    println!("plist:     {}", plist.display());
    println!("installed: {}", yes_no(installed));
    println!("running:   {}", yes_no(running));
    println!("probe:     {}", yes_no(probe_running(server).await));
    println!("log file:  {}", service_log_path(server).display());
    if !installed {
        println!(
            "hint:      register it with `fxg service install{}`",
            server_flag(server)
        );
    }
    Ok(())
}

// ----------------------------------------------------------------------
// Shared helpers
// ----------------------------------------------------------------------

/// Check whether the service actually responds (an OS-independent probe).
///
/// - daemon: whether the local IPC endpoint (Named Pipe / UDS) is reachable
/// - server: whether a TCP connection to the listen addr in
///   `~/.flexagent/config.toml` succeeds
async fn probe_running(server: bool) -> bool {
    let env = fxg_protocol::config::process_env;
    if server {
        let global = match fxg_protocol::config::GlobalConfig::load(&env) {
            Ok(global) => global,
            Err(_) => return false,
        };
        let addr = global.server.resolved_listen_addr().to_owned();
        let addr = addr
            .rsplit_once(':')
            .map(|(host, port)| {
                let host = if host.is_empty() || host == "0.0.0.0" {
                    "127.0.0.1"
                } else {
                    host
                };
                format!("{host}:{port}")
            })
            .unwrap_or(addr);
        let socket = match addr.parse() {
            Ok(socket) => socket,
            Err(_) => return false,
        };
        std::net::TcpStream::connect_timeout(&socket, std::time::Duration::from_millis(500)).is_ok()
    } else {
        crate::client::DaemonClient::connect().await.is_ok()
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[allow(dead_code)]
fn server_flag(server: bool) -> &'static str {
    if server { " --server" } else { "" }
}

fn service_log_path(server: bool) -> PathBuf {
    let env = fxg_protocol::config::process_env;
    let home = fxg_protocol::config::fxg_home(&env);
    let name = if server { "server.log" } else { "daemon.log" };
    home.join("logs").join(name)
}
