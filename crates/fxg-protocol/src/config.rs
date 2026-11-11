//! Process bootstrap configuration (`~/.flexagent/config.toml`).
//!
//! Design: `docs/05-cli-and-pwa-ui.md` §2 and
//! `docs/changelog/2026-11-09-settings-consolidation-and-project-identity.md`.
//!
//! Only values that are needed **before** the DB opens live here: bind addresses,
//! the hub URL and logging. Everything changed at runtime (node
//! settings, agent definitions, provisioners, project profiles, secrets) is owned
//! by `node.db` / `hub.db` and edited through the client API.
//!
//! These are hub/CLI internal types and are excluded from `specta` export
//! (the convention is that only types exposed to clients get
//! `#[specta(collect)]`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Environment variable that changes the data directory (`~/.flexagent`).
pub const ENV_FXG_HOME: &str = "FXG_HOME";
/// Global configuration file name.
pub const CONFIG_FILE_NAME: &str = "config.toml";
/// File name of the client authentication token.
pub const AUTH_TOKEN_FILE_NAME: &str = "auth_token";
/// File name of the per-node pairing token.
pub const NODE_TOKEN_FILE_NAME: &str = "node_token";
/// File name of the node-local DB.
pub const NODE_DB_FILE_NAME: &str = "node.db";
/// File name of the hub DB.
pub const HUB_DB_FILE_NAME: &str = "hub.db";
/// Definitions of the `FXG_*` environment variable names.
pub mod env_keys {
    /// `[node] node_id`
    pub const NODE_ID: &str = "FXG_NODE_ID";
    /// `[node] listen_addr`
    pub const NODE_LISTEN_ADDR: &str = "FXG_NODE_LISTEN_ADDR";
    /// Disable automatic `fxg node start` startup from CLI commands (`1` / `true`)
    pub const NO_AUTO_NODE: &str = "FXG_NO_AUTO_NODE";
    /// Idle seconds before a warm ACP agent process is disposed (0 = keep until shutdown)
    pub const AGENT_WARM_IDLE_SECS: &str = "FXG_AGENT_WARM_IDLE_SECS";
    /// `[hub] listen_addr`
    pub const HUB_LISTEN_ADDR: &str = "FXG_HUB_LISTEN_ADDR";
    /// `[hub] allowed_hosts` (comma separated)
    pub const HUB_ALLOWED_HOSTS: &str = "FXG_HUB_ALLOWED_HOSTS";
    /// `[hub] allowed_origins` (comma separated)
    pub const HUB_ALLOWED_ORIGINS: &str = "FXG_HUB_ALLOWED_ORIGINS";
    /// `[log] level` ("trace" | "debug" | "info" | "warn" | "error")
    pub const LOG_LEVEL: &str = "FXG_LOG_LEVEL";
    /// `[log] file`
    pub const LOG_FILE: &str = "FXG_LOG_FILE";
    /// `[log] format` ("text" | "json")
    pub const LOG_FORMAT: &str = "FXG_LOG_FORMAT";
    /// `[log] no_color`
    pub const LOG_NO_COLOR: &str = "FXG_LOG_NO_COLOR";
}

/// Path of the Node Hub WebSocket endpoint on a hub.
pub const NODE_WS_PATH: &str = "/api/v1/node/ws";

/// Returns a leftover `[node] hub_url` value in `config.toml`, if any.
///
/// The key is no longer part of the schema (the pairing moved to `node.db`), but
/// a silently ignored connection target is confusing, so callers warn when it is
/// still present. The value is never used.
pub fn legacy_hub_url(path: &Path) -> Option<String> {
    // Diagnostic only: an unreadable or malformed file is reported by the
    // regular configuration loader, so errors are ignored here.
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return None,
    };
    let document: toml::Value = match toml::from_str(&text) {
        Ok(document) => document,
        Err(_) => return None,
    };
    document
        .get("node")?
        .get("hub_url")?
        .as_str()
        .map(str::to_owned)
}

/// Error raised while loading or parsing configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Failed to read the configuration file
    #[error("failed to read config file {path}: {source}")]
    Io {
        /// Affected path
        path: PathBuf,
        /// Original I/O error
        #[source]
        source: std::io::Error,
    },
    /// Failed to parse the configuration file as TOML
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        /// Affected path
        path: PathBuf,
        /// Original parse error
        #[source]
        source: Box<toml::de::Error>,
    },
    /// Invalid environment variable value
    #[error("invalid value for environment variable {key}: {value:?}")]
    InvalidEnvValue {
        /// Environment variable name
        key: &'static str,
        /// The invalid value
        value: String,
    },
    /// Invalid hub address (not an `http(s)://` or `ws(s)://` URL)
    #[error("invalid hub address {value:?}: {reason}")]
    InvalidHubUrl {
        /// The invalid value
        value: String,
        /// Why it was rejected
        reason: String,
    },
}

/// Environment variable lookup function.
///
/// So that tests can substitute it, the configuration loader always reads
/// environment variables through this function ([`process_env`] is the real
/// implementation).
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// [`EnvLookup`] implementation that reads the real process environment.
#[allow(
    clippy::manual_ok_err,
    reason = "Result::ok is disallowed here to keep error-discarding explicit"
)]
pub fn process_env(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) => Some(value),
        Err(_) => None,
    }
}

/// Resolves the FlexAgent data directory (`~/.flexagent`).
///
/// If the [`ENV_FXG_HOME`] environment variable is set it takes precedence;
/// otherwise `~/.flexagent` is returned (`./.flexagent` if the home directory
/// cannot be resolved).
pub fn fxg_home(env: EnvLookup<'_>) -> PathBuf {
    if let Some(value) = env(ENV_FXG_HOME).filter(|v| !v.trim().is_empty()) {
        return PathBuf::from(value);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".flexagent")
}

fn parse_bool_env(key: &'static str, value: &str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidEnvValue {
            key,
            value: value.to_owned(),
        }),
    }
}

fn parse_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Converts a user-supplied hub address into the Node Hub WebSocket URL.
///
/// `http://host:port` and `https://host:port` are accepted so users do not have
/// to know the internal `/api/v1/node/ws` endpoint. A full `ws(s)://` URL is
/// used as-is (the endpoint path is appended when it is missing).
pub fn node_ws_url(input: &str) -> Result<String, ConfigError> {
    let value = input.trim().trim_end_matches('/');
    let (scheme, rest) = value
        .split_once("://")
        .ok_or_else(|| ConfigError::InvalidHubUrl {
            value: input.to_owned(),
            reason: "expected http://, https://, ws:// or wss://".to_owned(),
        })?;
    let scheme_lower = scheme.to_ascii_lowercase();
    let scheme = match scheme_lower.as_str() {
        "http" | "ws" => "ws",
        "https" | "wss" => "wss",
        other => {
            return Err(ConfigError::InvalidHubUrl {
                value: input.to_owned(),
                reason: format!("unsupported scheme {other:?}"),
            });
        }
    };
    let rest = rest
        .split(['?', '#'])
        .next()
        .unwrap_or(rest)
        .trim_end_matches('/');
    if rest.is_empty() || rest.starts_with('/') {
        return Err(ConfigError::InvalidHubUrl {
            value: input.to_owned(),
            reason: "missing host".to_owned(),
        });
    }
    if rest.ends_with(NODE_WS_PATH) {
        Ok(format!("{scheme}://{rest}"))
    } else {
        Ok(format!("{scheme}://{rest}{NODE_WS_PATH}"))
    }
}

/// Returns the HTTP(S) address of a hub for display (`ws://.../api/v1/node/ws` →
/// `http://...`). Unrecognized values are returned unchanged.
pub fn hub_http_origin(input: &str) -> String {
    let value = input.trim();
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_owned();
    };
    let scheme_lower = scheme.to_ascii_lowercase();
    let scheme = match scheme_lower.as_str() {
        "ws" => "http",
        "wss" => "https",
        other => other,
    };
    let rest = rest
        .trim_end_matches('/')
        .strip_suffix(NODE_WS_PATH)
        .unwrap_or_else(|| rest.trim_end_matches('/'));
    format!("{scheme}://{rest}")
}

/// Global bootstrap configuration (`~/.flexagent/config.toml`).
///
/// Every field is optional (each has a default value).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    /// Logging settings
    pub log: LogConfig,
    /// Local node daemon bootstrap settings (`fxg node start`)
    pub node: NodeBootstrapConfig,
    /// Hub bootstrap settings (`fxg hub`)
    pub hub: HubBootstrapConfig,
}

impl GlobalConfig {
    /// Reads `config.toml` from the [data directory](fxg_home) using the real
    /// environment variables and applies the `FXG_*` overrides.
    ///
    /// Returns the default configuration when the file does not exist.
    pub fn load(env: EnvLookup<'_>) -> Result<Self, ConfigError> {
        let home = fxg_home(env);
        Self::load_from_dir(&home, env)
    }

    /// Reads `config.toml` from the given directory and applies the `FXG_*` overrides.
    pub fn load_from_dir(dir: &Path, env: EnvLookup<'_>) -> Result<Self, ConfigError> {
        let mut config = Self::load_from_path(&dir.join(CONFIG_FILE_NAME))?;
        config.apply_env_overrides(env)?;
        Ok(config)
    }

    /// Reads `config.toml` at the given path.
    ///
    /// Returns the default configuration when the file does not exist.
    pub fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|source| ConfigError::Parse {
                path: path.to_path_buf(),
                source: Box::new(source),
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Applies the overrides from the `FXG_*` environment variables.
    pub fn apply_env_overrides(&mut self, env: EnvLookup<'_>) -> Result<(), ConfigError> {
        if let Some(v) = env(env_keys::LOG_LEVEL) {
            self.log.level = Some(v);
        }
        if let Some(v) = env(env_keys::LOG_FILE) {
            self.log.file = Some(v);
        }
        if let Some(v) = env(env_keys::LOG_FORMAT) {
            self.log.format = Some(v);
        }
        if let Some(v) = env(env_keys::LOG_NO_COLOR) {
            self.log.no_color = parse_bool_env(env_keys::LOG_NO_COLOR, &v)?;
        }
        if let Some(v) = env(env_keys::NODE_ID) {
            self.node.node_id = Some(v);
        }
        if let Some(v) = env(env_keys::NODE_LISTEN_ADDR) {
            self.node.listen_addr = Some(v);
        }
        if let Some(v) = env(env_keys::HUB_LISTEN_ADDR) {
            self.hub.listen_addr = Some(v);
        }
        if let Some(v) = env(env_keys::HUB_ALLOWED_HOSTS) {
            self.hub.allowed_hosts = parse_list(&v);
        }
        if let Some(v) = env(env_keys::HUB_ALLOWED_ORIGINS) {
            self.hub.allowed_origins = parse_list(&v);
        }
        Ok(())
    }
}

/// The `[log]` section: logging settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// Default log level ("trace" | "debug" | "info" | "warn" | "error")
    pub level: Option<String>,
    /// Log file destination (relative paths start at ~/.flexagent; an empty string disables it)
    pub file: Option<String>,
    /// Log output format ("text" | "json")
    pub format: Option<String>,
    /// Whether to disable ANSI color output
    pub no_color: bool,
}

/// The `[node]` section: local node daemon bootstrap settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeBootstrapConfig {
    /// Unique node identifier
    /// (when omitted, a hostname-based slug plus a short ID is assigned on first start).
    pub node_id: Option<String>,
    /// Bind address of the local HTTP/WS server
    /// (a loopback address is recommended by the security principles).
    pub listen_addr: Option<String>,
}

impl NodeBootstrapConfig {
    /// Default bind address of the local HTTP/WS server (strictly bound to loopback).
    pub const DEFAULT_LISTEN_ADDR: &'static str = "127.0.0.1:7860";

    /// Returns the effective bind address.
    pub fn resolved_listen_addr(&self) -> &str {
        self.listen_addr
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(Self::DEFAULT_LISTEN_ADDR)
    }
}

/// The `[hub]` section: hub bootstrap settings.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HubBootstrapConfig {
    /// HTTP/WS bind address of the hub (listen only inside a LAN / VPN).
    pub listen_addr: Option<String>,
    /// Extra host names allowed by the DNS rebinding defense (Host header validation).
    pub allowed_hosts: Vec<String>,
    /// Extra origins allowed by the CSWSH defense (WebSocket Origin validation) besides the same origin.
    pub allowed_origins: Vec<String>,
}

impl HubBootstrapConfig {
    /// Default bind address of the hub.
    ///
    /// Never expose it directly to the public internet; listen only inside a LAN
    /// isolated by a firewall / VPN.
    pub const DEFAULT_LISTEN_ADDR: &'static str = "0.0.0.0:8080";

    /// Returns the effective bind address.
    pub fn resolved_listen_addr(&self) -> &str {
        self.listen_addr
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(Self::DEFAULT_LISTEN_ADDR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    const SAMPLE_GLOBAL_CONFIG: &str = r#"
[node]
node_id = "home-win"
listen_addr = "127.0.0.1:7860"

[hub]
listen_addr = "0.0.0.0:8080"
allowed_hosts = ["home-server.tailnet-xxxx.ts.net:8080", "192.168.1.50:8080"]
allowed_origins = ["https://home-server.tailnet-xxxx.ts.net"]
"#;

    #[test]
    fn parses_full_global_config() {
        let config: GlobalConfig = toml::from_str(SAMPLE_GLOBAL_CONFIG).unwrap();
        assert_eq!(config.node.node_id.as_deref(), Some("home-win"));
        assert_eq!(
            config.node.resolved_listen_addr(),
            NodeBootstrapConfig::DEFAULT_LISTEN_ADDR
        );
        assert_eq!(
            config.hub.resolved_listen_addr(),
            HubBootstrapConfig::DEFAULT_LISTEN_ADDR
        );
        assert_eq!(config.hub.allowed_hosts.len(), 2);
        assert_eq!(config.hub.allowed_origins.len(), 1);
    }

    #[test]
    fn defaults_are_applied_when_sections_are_missing() {
        let config: GlobalConfig = toml::from_str("").unwrap();
        assert_eq!(config, GlobalConfig::default());
    }

    #[test]
    fn unknown_fields_are_ignored_for_forward_compatibility() {
        let config: GlobalConfig = toml::from_str(
            r#"
[node]
node_id = "n1"
future_option = "whatever"
"#,
        )
        .unwrap();
        assert_eq!(config.node.node_id.as_deref(), Some("n1"));
    }

    #[test]
    fn env_overrides_are_applied() {
        let mut config: GlobalConfig = toml::from_str(SAMPLE_GLOBAL_CONFIG).unwrap();
        let env = env_map(&[
            ("FXG_NODE_ID", "override-node"),
            ("FXG_NODE_LISTEN_ADDR", "127.0.0.1:9000"),
            ("FXG_HUB_LISTEN_ADDR", "0.0.0.0:9090"),
            ("FXG_HUB_ALLOWED_HOSTS", "a.test, b.test ,, c.test"),
            ("FXG_HUB_ALLOWED_ORIGINS", "http://a.test"),
            ("FXG_LOG_LEVEL", "debug"),
            ("FXG_LOG_FILE", "custom.log"),
            ("FXG_LOG_FORMAT", "json"),
            ("FXG_LOG_NO_COLOR", "true"),
        ]);
        config
            .apply_env_overrides(&|key| env.get(key).cloned())
            .unwrap();

        assert_eq!(config.log.level.as_deref(), Some("debug"));
        assert_eq!(config.log.file.as_deref(), Some("custom.log"));
        assert_eq!(config.log.format.as_deref(), Some("json"));
        assert!(config.log.no_color);
        assert_eq!(config.node.node_id.as_deref(), Some("override-node"));
        assert_eq!(config.node.resolved_listen_addr(), "127.0.0.1:9000");
        assert_eq!(config.hub.resolved_listen_addr(), "0.0.0.0:9090");
        assert_eq!(config.hub.allowed_hosts, vec!["a.test", "b.test", "c.test"]);
        assert_eq!(config.hub.allowed_origins, vec!["http://a.test"]);
    }

    #[test]
    fn fxg_home_prefers_env_then_falls_back_to_home_dir() {
        let env = env_map(&[("FXG_HOME", "/tmp/fxg-test-home")]);
        assert_eq!(
            fxg_home(&|key| env.get(key).cloned()),
            PathBuf::from("/tmp/fxg-test-home")
        );

        let empty = env_map(&[]);
        let fallback = fxg_home(&|key| empty.get(key).cloned());
        assert!(fallback.ends_with(".flexagent"));
    }

    #[test]
    fn missing_config_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = GlobalConfig::load_from_dir(dir.path(), &|_| None).unwrap();
        assert_eq!(config, GlobalConfig::default());
    }

    #[test]
    fn loads_config_file_from_directory_with_env_overrides() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(CONFIG_FILE_NAME), SAMPLE_GLOBAL_CONFIG).unwrap();
        let env = env_map(&[("FXG_NODE_ID", "from-env")]);
        let config = GlobalConfig::load_from_dir(dir.path(), &|key| env.get(key).cloned()).unwrap();
        assert_eq!(config.node.node_id.as_deref(), Some("from-env"));
    }

    #[test]
    fn invalid_toml_reports_parse_error_with_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE_NAME);
        std::fs::write(&path, "this is not = valid toml [").unwrap();
        let err = GlobalConfig::load_from_path(&path).unwrap_err();
        match err {
            ConfigError::Parse { path: p, .. } => assert_eq!(p, path),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn node_ws_url_derives_the_endpoint_from_an_http_address() {
        assert_eq!(
            node_ws_url("http://localhost:8080").unwrap(),
            "ws://localhost:8080/api/v1/node/ws"
        );
        assert_eq!(
            node_ws_url("https://hub.example.com/").unwrap(),
            "wss://hub.example.com/api/v1/node/ws"
        );
        assert_eq!(
            node_ws_url("ws://localhost:8080").unwrap(),
            "ws://localhost:8080/api/v1/node/ws"
        );
        assert_eq!(
            node_ws_url("wss://hub.example.com/api/v1/node/ws").unwrap(),
            "wss://hub.example.com/api/v1/node/ws"
        );
    }

    #[test]
    fn node_ws_url_rejects_unknown_schemes_and_missing_hosts() {
        assert!(matches!(
            node_ws_url("localhost:8080"),
            Err(ConfigError::InvalidHubUrl { .. })
        ));
        assert!(matches!(
            node_ws_url("ftp://localhost:8080"),
            Err(ConfigError::InvalidHubUrl { .. })
        ));
        assert!(matches!(
            node_ws_url("http://"),
            Err(ConfigError::InvalidHubUrl { .. })
        ));
    }

    #[test]
    fn hub_http_origin_hides_the_websocket_path() {
        assert_eq!(
            hub_http_origin("ws://localhost:8080/api/v1/node/ws"),
            "http://localhost:8080"
        );
        assert_eq!(
            hub_http_origin("wss://hub.example.com/api/v1/node/ws/"),
            "https://hub.example.com"
        );
        assert_eq!(
            hub_http_origin("http://localhost:8080"),
            "http://localhost:8080"
        );
    }
}
