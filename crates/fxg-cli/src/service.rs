//! `fxg service` — OS ログイン時のバックグラウンド常駐サービス管理。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §5.4、`docs/05-cli-and-pwa-ui.md` §1。
//!
//! - **Windows**: タスクスケジューラ (ログオン時実行。コンソールウィンドウは
//!   表示されない)
//! - **Linux / WSL**: `~/.config/systemd/user/<name>.service` +
//!   `systemctl --user`
//! - **macOS**: `~/Library/LaunchAgents/dev.flexagent.<name>.plist` +
//!   `launchctl`
//!
//! `--server` 指定時はデーモンではなく中央サーバーをサービス化する。

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// サービスの動作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    /// サービスを登録する (自動起動を有効化)
    Install,
    /// サービス登録を解除する
    Uninstall,
    /// サービスを開始する
    Start,
    /// サービスを停止する
    Stop,
    /// サービスを再起動する
    Restart,
    /// サービスの状態を表示する
    Status,
}

impl ServiceAction {
    /// CLI の `<action>` 文字列を解析する。
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

/// サービス名 (`fxg-daemon` / `fxg-server`)。
fn service_name(server: bool) -> &'static str {
    if server { "fxg-server" } else { "fxg-daemon" }
}

/// サービスが実行するサブコマンド引数。
fn service_args(server: bool) -> &'static [&'static str] {
    if server { &["server"] } else { &["daemon"] }
}

/// `fxg service <action>` を実行する。
pub async fn run(action: ServiceAction, server: bool) -> Result<()> {
    if action == ServiceAction::Status {
        let exe = std::env::current_exe().context("failed to resolve the fxg executable path")?;
        return status(server, &exe).await;
    }
    run_sync(action, server)
}

/// `fxg service <action>` の同期コア (`status` を除く。CLI 表示なし)。
///
/// トレイメニューの「自動起動」トグルからも同じ登録/解除ロジックを再利用する。
pub fn run_sync(action: ServiceAction, server: bool) -> Result<()> {
    debug_assert!(
        action != ServiceAction::Status,
        "status is handled by `run`"
    );
    let name = service_name(server);
    let exe = std::env::current_exe().context("failed to resolve the fxg executable path")?;
    platform_run(action, server, name, &exe)
}

/// 自動起動が登録されているか (トレイのチェック表示用。副作用なし)。
#[cfg(windows)]
pub fn is_installed(server: bool) -> bool {
    let name = service_name(server);
    task_exists(name)
        || startup_script_path(name)
            .map(|path| path.exists())
            .unwrap_or(false)
}

/// 自動起動が登録されているか (systemd ユニットファイルの有無)。
#[cfg(target_os = "linux")]
pub fn is_installed(server: bool) -> bool {
    unit_path(service_name(server))
        .map(|path| path.exists())
        .unwrap_or(false)
}

/// 自動起動が登録されているか (LaunchAgent plist の有無)。
#[cfg(target_os = "macos")]
pub fn is_installed(server: bool) -> bool {
    plist_path(service_name(server))
        .map(|path| path.exists())
        .unwrap_or(false)
}

// ----------------------------------------------------------------------
// Windows: タスクスケジューラ
// ----------------------------------------------------------------------

#[cfg(windows)]
fn platform_run(
    action: ServiceAction,
    server: bool,
    name: &str,
    exe: &std::path::Path,
) -> Result<()> {
    // `/TR "\"<exe>\" <sub>"` の形へ組み立てる (schtasks は空白パスを引用符で受ける)
    let args = service_args(server).join(" ");
    let task_run = format!("\"\\\"{}\\\" {args}\"", exe.display());
    let startup_script = startup_script_path(name)?;

    match action {
        ServiceAction::Install => {
            // まずタスクスケジューラを試す。管理者権限が無い環境では
            // スタートアップフォルダ (VBS ランチャー) へフォールバックする。
            match schtasks(&[
                "/Create", "/F", "/SC", "ONLOGON", "/TN", name, "/TR", &task_run,
            ]) {
                Ok(()) => {
                    println!("サービスを登録しました (タスクスケジューラ / ログオン時): {name}");
                }
                Err(err) => {
                    eprintln!("タスクスケジューラへの登録に失敗しました: {err}");
                    write_startup_script(&startup_script, exe, server)?;
                    println!(
                        "サービスを登録しました (スタートアップフォルダ): {}",
                        startup_script.display()
                    );
                }
            }
        }
        ServiceAction::Uninstall => {
            let _ = schtasks(&["/End", "/TN", name]);
            let task_removed = schtasks(&["/Delete", "/F", "/TN", name]).is_ok();
            let script_removed = if startup_script.exists() {
                std::fs::remove_file(&startup_script)?;
                true
            } else {
                false
            };
            if !task_removed && !script_removed {
                bail!("サービス登録が見つかりません: {name}");
            }
            println!("サービス登録を解除しました: {name}");
        }
        ServiceAction::Start => {
            if task_exists(name) {
                schtasks(&["/Run", "/TN", name])?;
            } else if startup_script.exists() {
                start_via_script(&startup_script)?;
            } else {
                bail!("サービスが登録されていません。`fxg service install` を実行してください");
            }
            println!("サービスを開始しました: {name}");
        }
        ServiceAction::Stop => {
            if task_exists(name) {
                schtasks(&["/End", "/TN", name])?;
            } else {
                stop_by_command_line(exe, service_args(server))?;
            }
            println!("サービスを停止しました: {name}");
        }
        ServiceAction::Restart => {
            if task_exists(name) {
                let _ = schtasks(&["/End", "/TN", name]);
                schtasks(&["/Run", "/TN", name])?;
            } else if startup_script.exists() {
                let _ = stop_by_command_line(exe, service_args(server));
                start_via_script(&startup_script)?;
            } else {
                bail!("サービスが登録されていません。`fxg service install` を実行してください");
            }
            println!("サービスを再起動しました: {name}");
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

/// スタートアップフォルダの VBS ランチャーパス (`%APPDATA%\...\Startup\<name>.vbs`)。
#[cfg(windows)]
fn startup_script_path(name: &str) -> Result<PathBuf> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .context("APPDATA が設定されていません")?;
    Ok(appdata
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join(format!("{name}.vbs")))
}

/// コンソールウィンドウを表示せずに起動する VBS ランチャーを書き出す。
#[cfg(windows)]
fn write_startup_script(path: &std::path::Path, exe: &std::path::Path, server: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let args = service_args(server).join(" ");
    // 第2引数 0 = ウィンドウ非表示 (CREATE_NO_WINDOW 相当)
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

/// 実行中の fxg プロセス (引数一致) を PowerShell CIM 経由で停止する。
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
    if !installed {
        println!(
            "hint:      `fxg service install{}` で登録できます",
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
            println!("サービスを登録しました: {} ({})", name, unit.display());
        }
        ServiceAction::Uninstall => {
            let _ = systemctl(&["disable", "--now", name]);
            if unit.exists() {
                std::fs::remove_file(&unit)?;
            }
            let _ = systemctl(&["daemon-reload"]);
            println!("サービス登録を解除しました: {name}");
        }
        ServiceAction::Start => {
            systemctl(&["start", name])?;
            println!("サービスを開始しました: {name}");
        }
        ServiceAction::Stop => {
            systemctl(&["stop", name])?;
            println!("サービスを停止しました: {name}");
        }
        ServiceAction::Restart => {
            systemctl(&["restart", name])?;
            println!("サービスを再起動しました: {name}");
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
        .context("failed to run systemctl (systemd --user が必要です)")?;
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
    if !installed {
        println!(
            "hint:      `fxg service install{}` で登録できます",
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
            println!("サービスを登録しました: {} ({})", label, plist.display());
        }
        ServiceAction::Uninstall => {
            let _ = launchctl(&["unload", "-w", &plist.display().to_string()]);
            if plist.exists() {
                std::fs::remove_file(&plist)?;
            }
            println!("サービス登録を解除しました: {label}");
        }
        ServiceAction::Start => {
            launchctl(&["load", "-w", &plist.display().to_string()])?;
            println!("サービスを開始しました: {label}");
        }
        ServiceAction::Stop => {
            let target = format!("gui/{}", uid());
            launchctl(&["kill", "SIGTERM", &format!("{target}/{label}")])?;
            println!("サービスを停止しました: {label}");
        }
        ServiceAction::Restart => {
            let target = format!("gui/{}", uid());
            launchctl(&["kickstart", "-k", &format!("{target}/{label}")])?;
            println!("サービスを再起動しました: {label}");
        }
        ServiceAction::Status => unreachable!("status is handled by the caller"),
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn uid() -> u32 {
    // SAFETY: `getuid` は常に成功する
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
    if !installed {
        println!(
            "hint:      `fxg service install{}` で登録できます",
            server_flag(server)
        );
    }
    Ok(())
}

// ----------------------------------------------------------------------
// 共通ヘルパー
// ----------------------------------------------------------------------

/// サービスが実際に応答しているかを確認する (OS 非依存の疎通確認)。
///
/// - daemon: ローカルIPC (Named Pipe / UDS) へ接続できるか
/// - server: `~/.flexagent/config.toml` の listen addr へ TCP 接続できるか
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
