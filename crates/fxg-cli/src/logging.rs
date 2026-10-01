//! FlexAgent ロギング基盤。
//!
//! - stderr (Console): TTY 判定および `NO_COLOR` 環境変数に基づき ANSI カラーを自動制御
//! - file (Logs): ANSI カラーを強制排除し、ミリ秒精度の ISO 8601 タイムスタンプで保存
//! - 起動バナー: デーモン・サーバーフォアグラウンド実行時に接続先や状態を出力

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use fxg_protocol::config::{GlobalConfig, LogConfig};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// サブコマンドの種類に応じたロギング種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandLogKind {
    /// 常駐デーモン (`fxg daemon`)
    Daemon,
    /// 中央サーバー (`fxg server`)
    Server,
    /// 通常の CLI コマンド (`fxg ps`, `fxg session ...` 等)
    #[default]
    Cli,
}

/// ロギング初期化オプション。
#[derive(Debug, Clone, Default)]
pub struct LoggingOptions {
    /// コマンドラインで指定されたログレベル
    pub log_level: Option<String>,
    /// 詳細化 (-v: 1, -vv: 2)
    pub verbose: u8,
    /// 抑制 (-q)
    pub quiet: bool,
    /// カラー無効化フラグ
    pub no_color: bool,
    /// ログファイル出力先 (None: デフォルト動作, Some("-"): 出力なし)
    pub log_file: Option<String>,
    /// ログフォーマット ("text" | "json")
    pub log_format: Option<String>,
    /// コマンド種別
    pub kind: CommandLogKind,
    /// stdio モードか (一時VM)
    pub is_stdio: bool,
}

/// ANSI カラーを出力すべきかを判定する。
///
/// 1. `--no-color` または `config.no_color` が true なら無効
/// 2. `NO_COLOR` 環境変数が設定されていれば無効 (https://no-color.org/)
/// 3. `CLICOLOR_FORCE` が "0" 以外で設定されていれば有効
/// 4. それ以外は `stderr.is_terminal()` (ファイルやパイプへのリダイレクト時は自動で無効)
pub fn should_use_color(no_color_flag: bool, config_no_color: bool) -> bool {
    if no_color_flag || config_no_color {
        return false;
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if let Ok(force) = std::env::var("CLICOLOR_FORCE")
        && force != "0"
    {
        return true;
    }
    std::io::stderr().is_terminal()
}

/// ログレベルの文字列・フィルタを解決する。
fn resolve_filter(opts: &LoggingOptions, config: &LogConfig) -> EnvFilter {
    // 1. RUST_LOG 環境変数が最優先
    if let Ok(env_filter) = EnvFilter::try_from_default_env() {
        return env_filter;
    }

    // 2. CLI フラグによるレベル解決
    if opts.quiet {
        return EnvFilter::new("error");
    }
    if opts.verbose >= 2 {
        return EnvFilter::new("fxg=trace,trace");
    }
    if opts.verbose == 1 {
        return EnvFilter::new("fxg=debug,debug");
    }
    if let Some(level) = opts.log_level.as_deref() {
        let level = level.trim().to_ascii_lowercase();
        return EnvFilter::new(format!("fxg={level},{level}"));
    }

    // 3. 設定ファイル (config.toml) の level
    if let Some(level) = config.level.as_deref() {
        let level = level.trim().to_ascii_lowercase();
        return EnvFilter::new(format!("fxg={level},{level}"));
    }

    // 4. コマンド種別ごとのデフォルト
    match opts.kind {
        CommandLogKind::Daemon | CommandLogKind::Server => {
            // デーモン・サーバーは fxg 内部は info、サードパーティのノイズは warn
            EnvFilter::new("fxg=info,warn")
        }
        CommandLogKind::Cli => {
            // 通常の CLI は出力を邪魔しないよう warn
            EnvFilter::new("warn")
        }
    }
}

/// ログファイルの出力先パスを解決する。
pub fn resolve_log_file_path(
    opts: &LoggingOptions,
    config: &LogConfig,
    fxg_home: &Path,
) -> Option<PathBuf> {
    // 1. CLI オプション
    if let Some(path_str) = opts.log_file.as_deref() {
        if path_str == "-" || path_str.eq_ignore_ascii_case("none") || path_str.is_empty() {
            return None;
        }
        return Some(resolve_relative_path(path_str, fxg_home));
    }

    // 2. 設定ファイル
    if let Some(path_str) = config.file.as_deref() {
        if path_str == "-" || path_str.eq_ignore_ascii_case("none") || path_str.is_empty() {
            return None;
        }
        return Some(resolve_relative_path(path_str, fxg_home));
    }

    // 3. デフォルト動作:
    //    Daemon / Server の常駐プロセスかつ stdio 以外なら自動で ~/.flexagent/logs/<kind>.log
    if opts.is_stdio {
        return None;
    }
    match opts.kind {
        CommandLogKind::Daemon => Some(fxg_home.join("logs").join("daemon.log")),
        CommandLogKind::Server => Some(fxg_home.join("logs").join("server.log")),
        CommandLogKind::Cli => None,
    }
}

fn resolve_relative_path(path_str: &str, base: &Path) -> PathBuf {
    let path = PathBuf::from(path_str);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

/// ロギングを初期化する。
pub fn init_logging(
    opts: &LoggingOptions,
    global: &GlobalConfig,
    fxg_home: &Path,
) -> Option<PathBuf> {
    let use_color = should_use_color(opts.no_color, global.log.no_color);
    let filter = resolve_filter(opts, &global.log);
    let is_json = opts
        .log_format
        .as_deref()
        .or(global.log.format.as_deref())
        .map(|f| f.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    let log_file_path = resolve_log_file_path(opts, &global.log, fxg_home);

    // ログファイル出力レイヤー (ANSI カラーは強制無効)
    let file_layer = if let Some(ref path) = log_file_path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("daemon.log");
        let dir = path.parent().unwrap_or(fxg_home);
        let appender = tracing_appender::rolling::never(dir, file_name);

        if is_json {
            Some(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_ansi(false)
                    .with_writer(appender)
                    .boxed(),
            )
        } else {
            Some(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_target(true)
                    .with_writer(appender)
                    .boxed(),
            )
        }
    } else {
        None
    };

    // stderr レイヤー (TTY判定に基づくANSIカラー制御)
    let stderr_layer = if is_json {
        tracing_subscriber::fmt::layer()
            .json()
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .with_ansi(use_color)
            .with_target(opts.verbose > 0)
            .with_writer(std::io::stderr)
            .boxed()
    };

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer);

    let _ = subscriber.try_init();

    log_file_path
}

/// デーモン起動バナーを stderr に出力する。
///
/// ※ `--stdio` モード時はプロトコル保護のため呼ばない。
pub fn print_daemon_banner(
    node_id: &str,
    http_addr: &str,
    ipc_endpoint: &str,
    log_file: Option<&Path>,
    central_server_url: Option<&str>,
    auth_token: Option<&str>,
    use_color: bool,
) {
    let log_str = log_file
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "none".to_owned());
    let sync_str = central_server_url.unwrap_or("standalone (local only)");

    if use_color {
        eprintln!(
            "\x1b[1;36mFlexAgent daemon v{}\x1b[0m started",
            env!("CARGO_PKG_VERSION")
        );
        eprintln!("  \x1b[1mNode ID:\x1b[0m      {node_id}");
        eprintln!("  \x1b[1mHTTP/WS API:\x1b[0m  http://{http_addr}");
        if let Some(token) = auth_token {
            eprintln!("  \x1b[1mWeb UI:\x1b[0m       http://{http_addr}/?token={token}");
            eprintln!("  \x1b[1mAuth Token:\x1b[0m   \x1b[33m{token}\x1b[0m");
        } else {
            eprintln!("  \x1b[1mWeb UI:\x1b[0m       http://{http_addr}/");
        }
        eprintln!("  \x1b[1mIPC Endpoint:\x1b[0m {ipc_endpoint}");
        eprintln!("  \x1b[1mLog File:\x1b[0m     {log_str}");
        eprintln!("  \x1b[1mOutbox Sync:\x1b[0m  {sync_str}");
        eprintln!("  \x1b[2mPress Ctrl+C to stop.\x1b[0m\n");
    } else {
        eprintln!("FlexAgent daemon v{} started", env!("CARGO_PKG_VERSION"));
        eprintln!("  Node ID:      {node_id}");
        eprintln!("  HTTP/WS API:  http://{http_addr}");
        if let Some(token) = auth_token {
            eprintln!("  Web UI:       http://{http_addr}/?token={token}");
            eprintln!("  Auth Token:   {token}");
        } else {
            eprintln!("  Web UI:       http://{http_addr}/");
        }
        eprintln!("  IPC Endpoint: {ipc_endpoint}");
        eprintln!("  Log File:     {log_str}");
        eprintln!("  Outbox Sync:  {sync_str}");
        eprintln!("  Press Ctrl+C to stop.\n");
    }
}

/// サーバー起動バナーを stderr に出力する。
pub fn print_server_banner(
    http_addr: &str,
    fxg_home: &Path,
    log_file: Option<&Path>,
    auth_token: Option<&str>,
    use_color: bool,
) {
    let log_str = log_file
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "none".to_owned());

    if use_color {
        eprintln!(
            "\x1b[1;35mFlexAgent central server v{}\x1b[0m started",
            env!("CARGO_PKG_VERSION")
        );
        eprintln!("  \x1b[1mHTTP/WS API:\x1b[0m  http://{http_addr}");
        if let Some(token) = auth_token {
            eprintln!("  \x1b[1mWeb UI:\x1b[0m       http://{http_addr}/?token={token}");
            eprintln!("  \x1b[1mAuth Token:\x1b[0m   \x1b[33m{token}\x1b[0m");
        } else {
            eprintln!("  \x1b[1mWeb UI:\x1b[0m       http://{http_addr}/");
        }
        eprintln!("  \x1b[1mData Dir:\x1b[0m     {}", fxg_home.display());
        eprintln!("  \x1b[1mLog File:\x1b[0m     {log_str}");
        eprintln!("  \x1b[2mPress Ctrl+C to stop.\x1b[0m\n");
    } else {
        eprintln!(
            "FlexAgent central server v{} started",
            env!("CARGO_PKG_VERSION")
        );
        eprintln!("  HTTP/WS API:  http://{http_addr}");
        if let Some(token) = auth_token {
            eprintln!("  Web UI:       http://{http_addr}/?token={token}");
            eprintln!("  Auth Token:   {token}");
        } else {
            eprintln!("  Web UI:       http://{http_addr}/");
        }
        eprintln!("  Data Dir:     {}", fxg_home.display());
        eprintln!("  Log File:     {log_str}");
        eprintln!("  Press Ctrl+C to stop.\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_color_honors_explicit_flags() {
        assert!(!should_use_color(true, false));
        assert!(!should_use_color(false, true));
    }

    #[test]
    fn resolve_filter_respects_quiet_and_verbose() {
        let mut opts = LoggingOptions::default();
        let config = LogConfig::default();

        opts.quiet = true;
        let filter = resolve_filter(&opts, &config);
        assert_eq!(filter.to_string(), "error");

        opts.quiet = false;
        opts.verbose = 1;
        let filter = resolve_filter(&opts, &config);
        assert_eq!(filter.to_string(), "fxg=debug,debug");
    }

    #[test]
    fn resolve_log_file_path_handles_dash() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = LoggingOptions::default();
        let config = LogConfig::default();

        opts.log_file = Some("-".to_owned());
        assert_eq!(resolve_log_file_path(&opts, &config, dir.path()), None);

        opts.log_file = Some("custom.log".to_owned());
        assert_eq!(
            resolve_log_file_path(&opts, &config, dir.path()),
            Some(dir.path().join("custom.log"))
        );
    }

    #[test]
    fn print_banners_do_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        print_daemon_banner(
            "node_test",
            "127.0.0.1:7860",
            r"\\.\pipe\test",
            Some(dir.path()),
            None,
            Some("test_token_123"),
            true,
        );
        print_daemon_banner(
            "node_test",
            "127.0.0.1:7860",
            r"\\.\pipe\test",
            None,
            None,
            None,
            false,
        );
        print_server_banner(
            "0.0.0.0:8080",
            dir.path(),
            None,
            Some("test_token_123"),
            true,
        );
        print_server_banner("0.0.0.0:8080", dir.path(), None, None, false);
    }
}
