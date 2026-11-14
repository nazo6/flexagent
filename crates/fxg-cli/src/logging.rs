//! FlexAgent logging infrastructure.
//!
//! - stderr (Console): ANSI color is auto-controlled from the TTY check and the
//!   `NO_COLOR` environment variable
//! - file (Logs): ANSI color is stripped and records are stored with
//!   millisecond-precision ISO 8601 timestamps
//! - startup banner: prints endpoints and state when the daemon or server runs
//!   in the foreground

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use fxg_protocol::config::LogConfig;
use fxg_protocol::paths::Paths;
use nu_ansi_term::{Color, Style};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::output::{Banner, KvList};

/// Logging kind for the subcommand being run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandLogKind {
    /// Resident node (`fxg node start`)
    Node,
    /// Hub (`fxg hub start`)
    Hub,
    /// Ordinary CLI command (`fxg session list`, `fxg session ...`, etc.)
    #[default]
    Cli,
}

/// Logging initialization options.
#[derive(Debug, Clone, Default)]
pub struct LoggingOptions {
    /// Log level given on the command line
    pub log_level: Option<String>,
    /// Verbosity (-v: 1, -vv: 2)
    pub verbose: u8,
    /// Suppression (-q)
    pub quiet: bool,
    /// Flag that disables color
    pub no_color: bool,
    /// Log file destination (None: default behavior, Some("-"): no output)
    pub log_file: Option<String>,
    /// Log format ("text" | "json")
    pub log_format: Option<String>,
    /// Command kind
    pub kind: CommandLogKind,
    /// Whether stdio mode is active (ephemeral VM)
    pub is_stdio: bool,
}

/// Decide whether ANSI color should be emitted.
///
/// 1. Disabled when `--no-color` or `config.no_color` is true
/// 2. Disabled when the `NO_COLOR` environment variable is set (https://no-color.org/)
/// 3. Enabled when `CLICOLOR_FORCE` is set to anything other than "0"
/// 4. Otherwise `stderr.is_terminal()` (automatically disabled when redirected to
///    a file or pipe)
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

/// Resolve the log level string and filter.
fn resolve_filter(opts: &LoggingOptions, config: &LogConfig) -> EnvFilter {
    // 1. RUST_LOG environment variable has the highest priority
    if let Ok(env_filter) = EnvFilter::try_from_default_env() {
        return env_filter;
    }

    // 2. Level resolution from CLI flags
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

    // 3. `level` from the configuration file (node.toml / hub.toml)
    if let Some(level) = config.level.as_deref() {
        let level = level.trim().to_ascii_lowercase();
        return EnvFilter::new(format!("fxg={level},{level}"));
    }

    // 4. Default per command kind
    match opts.kind {
        CommandLogKind::Node | CommandLogKind::Hub => {
            // Daemon and server: info for fxg internals, warn for third-party noise
            EnvFilter::new("fxg=info,warn")
        }
        CommandLogKind::Cli => {
            // Ordinary CLI: warn so command output stays clean
            EnvFilter::new("warn")
        }
    }
}

/// Resolve the destination path of the log file.
pub fn resolve_log_file_path(
    opts: &LoggingOptions,
    config: &LogConfig,
    paths: &Paths,
) -> Option<PathBuf> {
    // 1. CLI option
    if let Some(path_str) = opts.log_file.as_deref() {
        if path_str == "-" || path_str.eq_ignore_ascii_case("none") || path_str.is_empty() {
            return None;
        }
        return Some(resolve_relative_path(path_str, &paths.log_dir()));
    }

    // 2. Configuration file
    if let Some(path_str) = config.file.as_deref() {
        if path_str == "-" || path_str.eq_ignore_ascii_case("none") || path_str.is_empty() {
            return None;
        }
        return Some(resolve_relative_path(path_str, &paths.log_dir()));
    }

    // 3. Default behavior:
    //    For a resident Node / Hub process outside stdio mode, use the role's log file.
    if opts.is_stdio {
        return None;
    }
    match opts.kind {
        CommandLogKind::Node | CommandLogKind::Hub => Some(paths.log_path()),
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

/// Initialize logging.
pub fn init_logging(opts: &LoggingOptions, config: &LogConfig, paths: &Paths) -> Option<PathBuf> {
    let use_color = should_use_color(opts.no_color, config.no_color);
    let filter = resolve_filter(opts, config);
    let is_json = opts
        .log_format
        .as_deref()
        .or(config.format.as_deref())
        .map(|f| f.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    let log_file_path = resolve_log_file_path(opts, config, paths);

    // Log file layer (ANSI color forcibly disabled)
    let file_layer = if let Some(ref path) = log_file_path {
        if let Some(parent) = path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            eprintln!(
                "fxg: failed to create the log directory {}: {err}",
                parent.display()
            );
        }
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("fxg.log");
        let fallback_dir = paths.log_dir();
        let dir = path.parent().unwrap_or(fallback_dir.as_path());
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

    // stderr layer (ANSI color controlled by the TTY check)
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

    if let Err(err) = subscriber.try_init() {
        // Another (test) subscriber may already be installed; logging still works.
        eprintln!("fxg: tracing subscriber was not installed: {err}");
    }

    log_file_path
}

/// Print the daemon startup banner to stderr.
///
/// Do not call this in `fxg internal stdio` mode, where it would corrupt the protocol.
pub fn print_node_banner(
    node_id: &str,
    http_addr: &str,
    log_file: Option<&Path>,
    hub_url: Option<&str>,
    auth_token: Option<&str>,
    use_color: bool,
) {
    eprintln!(
        "{}",
        render_node_banner(node_id, http_addr, log_file, hub_url, auth_token, use_color)
    );
}

fn render_node_banner(
    node_id: &str,
    http_addr: &str,
    log_file: Option<&Path>,
    hub_url: Option<&str>,
    auth_token: Option<&str>,
    use_color: bool,
) -> String {
    let log_str = log_file
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "none".to_owned());
    let sync_str = hub_url
        .map(fxg_protocol::config::hub_http_origin)
        .unwrap_or_else(|| "standalone (local only)".to_owned());

    let mut fields = KvList::new()
        .indent(2)
        .label_style(Style::new().bold())
        .field("Node ID:", node_id)
        .field("HTTP/WS API:", format!("http://{http_addr}"));
    if let Some(token) = auth_token {
        fields = fields
            .field("Web UI:", format!("http://{http_addr}/?token={token}"))
            .styled_field("Auth Token:", token, Color::Yellow.normal());
    } else {
        fields = fields.field("Web UI:", format!("http://{http_addr}/"));
    }
    fields = fields.field("Log File:", log_str).field("Hub:", sync_str);

    Banner::new(
        format!("FlexAgent node v{}", env!("CARGO_PKG_VERSION")),
        Color::Cyan.bold(),
        " started",
        fields,
    )
    .footer("Press Ctrl+C to stop.")
    .render(use_color)
}

/// Print the hub startup banner to stderr.
pub fn print_hub_banner(
    http_addr: &str,
    paths: &Paths,
    log_file: Option<&Path>,
    auth_token: Option<&str>,
    use_color: bool,
) {
    eprintln!(
        "{}",
        render_hub_banner(http_addr, paths, log_file, auth_token, use_color)
    );
}

fn render_hub_banner(
    http_addr: &str,
    paths: &Paths,
    log_file: Option<&Path>,
    auth_token: Option<&str>,
    use_color: bool,
) -> String {
    let log_str = log_file
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "none".to_owned());

    let mut fields = KvList::new()
        .indent(2)
        .label_style(Style::new().bold())
        .field("HTTP/WS API:", format!("http://{http_addr}"));
    if let Some(token) = auth_token {
        fields = fields
            .field("Web UI:", format!("http://{http_addr}/?token={token}"))
            .styled_field("Auth Token:", token, Color::Yellow.normal());
    } else {
        fields = fields.field("Web UI:", format!("http://{http_addr}/"));
    }
    fields = fields
        .field("Data Dir:", paths.data_dir().display().to_string())
        .field("Log File:", log_str);

    Banner::new(
        format!("FlexAgent hub v{}", env!("CARGO_PKG_VERSION")),
        Color::Purple.bold(),
        " started",
        fields,
    )
    .footer("Press Ctrl+C to stop.")
    .render(use_color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::paths::{Paths, Role};

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
        let paths = Paths::portable(dir.path(), Role::Node);
        let mut opts = LoggingOptions::default();
        let config = LogConfig::default();

        opts.log_file = Some("-".to_owned());
        assert_eq!(resolve_log_file_path(&opts, &config, &paths), None);

        opts.log_file = Some("custom.log".to_owned());
        assert_eq!(
            resolve_log_file_path(&opts, &config, &paths),
            Some(paths.log_dir().join("custom.log"))
        );
    }

    #[test]
    fn print_banners_do_not_panic_and_align_columns() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::portable(dir.path(), Role::Hub);

        let colored_node = render_node_banner(
            "node_test",
            "127.0.0.1:7860",
            Some(dir.path()),
            None,
            Some("test_token_123"),
            true,
        );
        let plain_node = render_node_banner(
            "node_test",
            "127.0.0.1:7860",
            Some(dir.path()),
            None,
            Some("test_token_123"),
            false,
        );
        assert!(!plain_node.contains("\x1b["));
        assert!(colored_node.contains("\x1b["));
        // Value column must start at column 16 (`  ` + 12 chars + 2 spaces) in plain output,
        // including `Hub:` after the longest label `HTTP/WS API:`.
        assert!(plain_node.contains("\n  Hub:          standalone (local only)\n"));
        assert!(colored_node.contains("\n  \x1b[1mHub:\x1b[0m          standalone (local only)\n"));

        print_node_banner("node_test", "127.0.0.1:7860", None, None, None, false);
        print_hub_banner("0.0.0.0:8080", &paths, None, Some("test_token_123"), true);
        print_hub_banner("0.0.0.0:8080", &paths, None, None, false);
    }
}
