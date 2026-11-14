//! Typed settings, secret metadata and project identity types.
//!
//! Design: `docs/proposals/2026-11-09-settings-consolidation-and-project-identity.md`.
//!
//! Settings are never sent as a generic key/value container. Each owner (node,
//! server, bootstrap file, browser) exposes its own typed snapshot, and the Web UI
//! renders a searchable catalog over those values. Secret values never appear in
//! these types: only metadata (`SecretSummary`) is exposed, and writes go through
//! a write-only request.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::SecretId;
use crate::config::EnvLookup;

crate::open_string_enum! {
    /// Where a bootstrap (process start-up) value came from.
    pub enum ConfigSource {
        /// Built-in default
        Default => "default",
        /// `node.toml` / `hub.toml`
        ConfigFile => "config_file",
        /// `FXG_*` environment variable
        Env => "env",
        /// CLI flag
        Cli => "cli",
    }
}

crate::open_string_enum! {
    /// How a logical project identity was derived.
    pub enum ProjectIdentityKind {
        /// Normalized `git remote.origin.url`
        GitRemote => "git_remote",
        /// Explicit `project_key` from `project_path_links`
        UserKey => "user_key",
        /// Machine-local fallback (`local:<node_id>:<path-hash>`)
        Local => "local",
    }
}

/// A read-only bootstrap value together with its origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SourcedValue<T> {
    /// The effective value
    pub value: T,
    /// Which layer supplied the value
    pub source: ConfigSource,
    /// Environment variable name / configuration key / CLI flag (when known)
    pub source_detail: Option<String>,
}

impl<T> SourcedValue<T> {
    /// Builds a value with its origin.
    pub fn new(value: T, source: ConfigSource, source_detail: Option<impl Into<String>>) -> Self {
        Self {
            value,
            source,
            source_detail: source_detail.map(Into::into),
        }
    }
}

/// Resolves the origin of an effective bootstrap string.
///
/// `effective` is the already merged value (CLI > env > config file). The
/// origin is attributed by checking the layers in the order environment →
/// configuration file → built-in default, falling back to the CLI flag.
///
/// The environment lookup is injected so tests do not depend on the process
/// environment (same convention as [`crate::config::NodeConfig`]).
pub fn sourced_string(
    env: EnvLookup<'_>,
    effective: String,
    env_key: &str,
    file_value: Option<String>,
    config_key: &str,
    default_value: Option<&str>,
    cli_flag: Option<&str>,
) -> SourcedValue<String> {
    let env_value = env(env_key);
    if env_value.as_deref() == Some(effective.as_str()) {
        return SourcedValue::new(effective, ConfigSource::Env, Some(env_key));
    }
    if file_value.as_deref() == Some(effective.as_str()) {
        return SourcedValue::new(effective, ConfigSource::ConfigFile, Some(config_key));
    }
    if env_value.is_none() && file_value.is_none() && default_value == Some(effective.as_str()) {
        return SourcedValue::new(effective, ConfigSource::Default, Option::<&str>::None);
    }
    SourcedValue::new(effective, ConfigSource::Cli, cli_flag)
}

/// Resolves the origin of an effective optional bootstrap string.
///
/// Unlike [`sourced_string`], `None` is the built-in default, so an absent
/// value is always attributed to the default layer.
pub fn sourced_option(
    env: EnvLookup<'_>,
    effective: Option<String>,
    env_key: &str,
    file_value: Option<String>,
    config_key: &str,
    cli_flag: Option<&str>,
) -> SourcedValue<Option<String>> {
    let Some(value) = effective else {
        return SourcedValue::new(None, ConfigSource::Default, Option::<&str>::None);
    };
    let env_value = env(env_key);
    if env_value.as_deref() == Some(value.as_str()) {
        return SourcedValue::new(Some(value), ConfigSource::Env, Some(env_key));
    }
    if file_value.as_deref() == Some(value.as_str()) {
        return SourcedValue::new(Some(value), ConfigSource::ConfigFile, Some(config_key));
    }
    SourcedValue::new(Some(value), ConfigSource::Cli, cli_flag)
}

/// Resolves the origin of an effective bootstrap string list.
///
/// `effective` is the already merged value (environment > config file). Lists
/// have no CLI layer today, so a value that matches neither the environment nor
/// the file is attributed to the built-in default (an empty list).
pub fn sourced_list(
    env: EnvLookup<'_>,
    effective: Vec<String>,
    env_key: &str,
    file_value: &[String],
    config_key: &str,
) -> SourcedValue<Vec<String>> {
    if env(env_key).is_some_and(|value| crate::config::parse_list(&value) == effective) {
        return SourcedValue::new(effective, ConfigSource::Env, Some(env_key));
    }
    if !effective.is_empty() && file_value == effective.as_slice() {
        return SourcedValue::new(effective, ConfigSource::ConfigFile, Some(config_key));
    }
    SourcedValue::new(effective, ConfigSource::Default, Option::<&str>::None)
}

/// Read-only bootstrap view of a local node (`node.toml` / env / CLI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct NodeBootstrapView {
    /// Node data directory (`<data>/node`)
    pub data_dir: SourcedValue<String>,
    /// Node ID
    pub node_id: SourcedValue<String>,
    /// Local HTTP/WS bind address
    pub listen_addr: SourcedValue<String>,
    /// Extra allowed `Host` values (DNS rebinding defense)
    pub allowed_hosts: SourcedValue<Vec<String>>,
    /// Extra allowed WebSocket `Origin` values (CSWSH defense)
    pub allowed_origins: SourcedValue<Vec<String>>,
    /// Log level
    pub log_level: SourcedValue<String>,
    /// Log file destination (empty when disabled)
    pub log_file: SourcedValue<Option<String>>,
    /// Log format (`text` / `json`)
    pub log_format: SourcedValue<String>,
    /// Whether ANSI colors are disabled
    pub log_no_color: SourcedValue<bool>,
}

/// Read-only bootstrap view of the hub (`hub.toml` / env / CLI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct HubBootstrapView {
    /// Hub data directory (`<data>/hub`)
    pub data_dir: SourcedValue<String>,
    /// HTTP/WS bind address
    pub listen_addr: SourcedValue<String>,
    /// Extra allowed `Host` values (DNS rebinding defense)
    pub allowed_hosts: SourcedValue<Vec<String>>,
    /// Extra allowed WebSocket `Origin` values (CSWSH defense)
    pub allowed_origins: SourcedValue<Vec<String>>,
    /// Log level
    pub log_level: SourcedValue<String>,
    /// Log file destination (empty when disabled)
    pub log_file: SourcedValue<Option<String>>,
    /// Log format (`text` / `json`)
    pub log_format: SourcedValue<String>,
    /// Whether ANSI colors are disabled
    pub log_no_color: SourcedValue<bool>,
}

/// Node-owned settings stored in `node.db` (`node_settings`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct NodeSettings {
    /// Display name in the UI
    pub name: String,
    /// Whether remote (through the hub) Web PTY sessions are allowed
    pub allow_remote_pty: bool,
    /// Directories scanned by `fxg project scan` when no directory is given
    pub project_scan_dirs: Vec<String>,
    /// Tray residency on start-up (`None` = default: disabled)
    pub tray: Option<bool>,
    /// Web Push contact claim (`mailto:` / `https:`) for standalone node Web Push
    pub vapid_subject: Option<String>,
    /// Optimistic-lock revision
    pub revision: u64,
    /// Last update time (Unix epoch ms)
    pub updated_at: i64,
}

/// `PATCH /api/v1/nodes/{node_id}/settings/node` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UpdateNodeSettingsRequest {
    /// Display name
    pub name: String,
    /// Remote PTY policy
    pub allow_remote_pty: bool,
    /// Default project scan directories
    pub project_scan_dirs: Vec<String>,
    /// Tray residency
    pub tray: Option<bool>,
    /// Web Push contact claim (`mailto:` / `https:`)
    pub vapid_subject: Option<String>,
    /// Revision the client observed (`REVISION_MISMATCH` on conflict)
    pub expected_revision: u64,
}

/// ACP client capabilities advertised when connecting to an agent.
///
/// Capabilities describe what this client understands, so an agent that does
/// not know a capability simply ignores it. Built-in agents use the defaults
/// (see also [`crate::client_api::AgentSummary::capabilities`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
#[serde(default)]
pub struct AgentAcpCapabilities {
    /// Advertise `clientCapabilities.subagents` (the ACP subagents RFD).
    pub subagents: bool,
}

impl Default for AgentAcpCapabilities {
    fn default() -> Self {
        Self { subagents: true }
    }
}

/// Node-owned agent settings stored in `node.db` (`agent_settings` / `agent_aliases`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentSettings {
    /// ACP Registry URL (`None` = built-in default)
    pub registry_url: Option<String>,
    /// Registry cache TTL in seconds (`None` = built-in default)
    pub registry_cache_ttl_secs: Option<u64>,
    /// Agent name aliases (merged over the built-in aliases)
    pub aliases: BTreeMap<String, String>,
    /// Optimistic-lock revision
    pub revision: u64,
    /// Last update time (Unix epoch ms)
    pub updated_at: i64,
}

impl AgentSettings {
    /// Default value of the official ACP registry URL.
    pub const DEFAULT_REGISTRY_URL: &'static str =
        "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
    /// Default value of the registry cache TTL (24 hours).
    pub const DEFAULT_REGISTRY_CACHE_TTL_SECS: u64 = 86_400;

    /// Returns the effective ACP registry URL.
    pub fn resolved_registry_url(&self) -> &str {
        self.registry_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(Self::DEFAULT_REGISTRY_URL)
    }

    /// Returns the effective registry cache TTL (seconds).
    pub fn resolved_registry_cache_ttl_secs(&self) -> u64 {
        self.registry_cache_ttl_secs
            .unwrap_or(Self::DEFAULT_REGISTRY_CACHE_TTL_SECS)
    }
}

/// `PATCH /api/v1/nodes/{node_id}/settings/agents` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UpdateAgentSettingsRequest {
    /// ACP Registry URL
    pub registry_url: Option<String>,
    /// Registry cache TTL in seconds
    pub registry_cache_ttl_secs: Option<u64>,
    /// Agent name aliases
    pub aliases: BTreeMap<String, String>,
    /// Revision the client observed (`REVISION_MISMATCH` on conflict)
    pub expected_revision: u64,
}

/// Complete settings snapshot of a local node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct NodeSettingsSnapshot {
    /// Read-only process bootstrap values
    pub bootstrap: NodeBootstrapView,
    /// Node-owned settings
    pub node: NodeSettings,
    /// Agent settings
    pub agents: AgentSettings,
    /// Hub pairing state
    pub pairing: PairingSettings,
}

/// Node ⇔ hub pairing state (`node.db` + the token file).
///
/// The pairing is owned by the node: `hub_url` is stored in `node.db` and the
/// token is a `0600` file (`<data>/node/node_token`). The client API never
/// returns the token, only whether it is configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct PairingSettings {
    /// Paired hub address (HTTP origin; `None` = standalone)
    pub hub_url: Option<String>,
    /// Whether the pairing token file exists
    pub token_configured: bool,
    /// Optimistic-lock revision
    pub revision: u64,
    /// Current node ⇔ hub link state
    pub connection: crate::client_api::HubLinkStatus,
    /// Whether the last `NodeHello` hit the session reporting limit
    pub sessions_truncated: bool,
}

/// `PUT /api/v1/nodes/{node_id}/settings/pairing` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UpdatePairingRequest {
    /// Hub address (`http://host:port`; a full `ws://.../api/v1/node/ws` URL is also accepted)
    pub hub_url: String,
    /// Pairing token (omit to keep the current one when the hub address is unchanged)
    pub token: Option<String>,
    /// Revision the client observed (`REVISION_MISMATCH` on conflict)
    pub expected_revision: u64,
}

/// `POST /api/v1/nodes/{node_id}/settings/pairing/test` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct TestPairingRequest {
    /// Hub address to try (`http://host:port` or a full WS URL)
    pub hub_url: String,
    /// Token to try (omit to use the configured one)
    pub token: Option<String>,
}

/// Hub-owned settings stored in `hub.db` (`hub_settings`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct HubSettings {
    /// Web Push contact claim (`mailto:` / `https:`)
    pub vapid_subject: Option<String>,
    /// Optimistic-lock revision
    pub revision: u64,
    /// Last update time (Unix epoch ms)
    pub updated_at: i64,
}

/// `PATCH /api/v1/settings/hub` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UpdateHubSettingsRequest {
    /// Web Push contact claim
    pub vapid_subject: Option<String>,
    /// Revision the client observed (`REVISION_MISMATCH` on conflict)
    pub expected_revision: u64,
}

/// Complete settings snapshot of the hub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct HubSettingsSnapshot {
    /// Read-only process bootstrap values
    pub bootstrap: HubBootstrapView,
    /// Hub-owned settings
    pub hub: HubSettings,
}

/// Where a custom agent definition came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[specta(collect)]
pub enum CustomAgentOrigin {
    /// Created manually through the UI or API.
    Manual,
    /// Imported from the ACP Registry catalog.
    Registry {
        /// Registry agent ID at import time.
        id: String,
        /// Registry version at import time.
        version: String,
    },
}

/// A custom ACP agent definition (`custom_agents` in `node.db`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[specta(collect)]
#[serde(default)]
pub struct CustomAgentConfig {
    /// Display name
    pub name: Option<String>,
    /// Command to execute
    pub command: String,
    /// Command arguments
    pub args: Vec<String>,
    /// Plain environment variables injected into the process (stored as-is in the DB)
    pub env: BTreeMap<String, String>,
    /// Environment variables resolved from the secret store at launch time
    pub env_secrets: BTreeMap<String, SecretId>,
    /// Built-in extension ID to apply on top of this custom launch.
    ///
    /// `None` keeps the agent ACP-only. The ID is validated against the node's
    /// registered extensions when the definition is saved.
    pub extension: Option<String>,
    /// Description shown in the agent list (copied from the registry on import).
    pub description: Option<String>,
    /// Icon URL or built-in asset key (copied from the registry on import).
    pub icon: Option<String>,
    /// Provenance of this definition (manual or imported from the registry).
    pub origin: Option<CustomAgentOrigin>,
    /// ACP client capabilities advertised on connections to this agent
    #[serde(default)]
    pub capabilities: AgentAcpCapabilities,
}

/// Definition of an ephemeral VM / sandbox provisioner (`provisioners` in `hub.db`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[specta(collect)]
#[serde(default)]
pub struct ProvisionerDefinition {
    /// Provisioner name (`local-docker` / `colab-pro` etc.)
    pub name: String,
    /// Description (shown in the UI)
    pub description: Option<String>,
    /// Command to execute (`docker` / `incus` / `uvx` / `ssh` etc.)
    pub command: String,
    /// Command arguments.
    ///
    /// Placeholders: `{INSTANCE_NAME}` / `{SESSION_ID}` / `{NODE_ID}`.
    pub args: Vec<String>,
    /// Seconds until the idle instance is discarded automatically
    pub idle_timeout_secs: Option<u64>,
    /// Environment variables injected into the process
    pub env: BTreeMap<String, String>,
    /// Environment variables resolved from the secret store at launch time
    pub env_secrets: BTreeMap<String, SecretId>,
}

/// Metadata of a stored user secret (never the value itself).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SecretSummary {
    /// Secret key / identifier (e.g. `ANTHROPIC_API_KEY`, `GH_TOKEN`)
    pub id: SecretId,
    /// Optional human-readable description or note
    pub description: Option<String>,
    /// Whether the value file exists
    pub configured: bool,
    /// Last update time (Unix epoch ms). `None` when the value is missing.
    pub updated_at: Option<i64>,
    /// Agent IDs / Git hosts referencing this secret (for `SECRET_IN_USE` diagnostics)
    pub used_by: Vec<String>,
}

/// `PUT /api/v1/{nodes/{node_id}/|server/}secrets/{secret_id}` request (write-only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UpsertSecretRequest {
    /// Optional description or note
    pub description: Option<String>,
    /// The plaintext value. Never echoed back in a response or written to logs.
    pub value: String,
}

/// Identity of a logical project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectIdentity {
    /// How the identity was derived
    pub kind: ProjectIdentityKind,
    /// Explicit project key (`Some` only for `UserKey`)
    pub key: Option<String>,
}

impl ProjectIdentity {
    /// Git-remote based identity.
    pub fn git_remote() -> Self {
        Self {
            kind: ProjectIdentityKind::GitRemote,
            key: None,
        }
    }

    /// Explicit user key identity.
    pub fn user_key(key: impl Into<String>) -> Self {
        Self {
            kind: ProjectIdentityKind::UserKey,
            key: Some(key.into()),
        }
    }

    /// Machine-local fallback identity.
    pub fn local() -> Self {
        Self {
            kind: ProjectIdentityKind::Local,
            key: None,
        }
    }
}

/// Response of the secret list endpoints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SecretsResponse {
    /// Secret metadata (values are never returned)
    pub secrets: Vec<SecretSummary>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::env_keys;

    fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn sourced_string_attributes_env_before_file() {
        let env = env_from(&[(env_keys::LOG_LEVEL, "debug")]);
        let value = sourced_string(
            &env,
            "debug".to_owned(),
            env_keys::LOG_LEVEL,
            Some("debug".to_owned()),
            "log.level",
            Some("info"),
            Some("--log-level"),
        );
        assert_eq!(value.source, ConfigSource::Env);
        assert_eq!(value.source_detail.as_deref(), Some(env_keys::LOG_LEVEL));
    }

    #[test]
    fn sourced_string_attributes_config_file() {
        let env = env_from(&[(env_keys::LOG_LEVEL, "debug")]);
        let value = sourced_string(
            &env,
            "warn".to_owned(),
            env_keys::LOG_LEVEL,
            Some("warn".to_owned()),
            "log.level",
            Some("info"),
            Some("--log-level"),
        );
        assert_eq!(value.source, ConfigSource::ConfigFile);
        assert_eq!(value.source_detail.as_deref(), Some("log.level"));
    }

    #[test]
    fn sourced_string_attributes_default_when_no_layer_is_set() {
        let env = env_from(&[]);
        let value = sourced_string(
            &env,
            "info".to_owned(),
            env_keys::LOG_LEVEL,
            None,
            "log.level",
            Some("info"),
            Some("--log-level"),
        );
        assert_eq!(value.source, ConfigSource::Default);
        assert_eq!(value.source_detail, None);
    }

    #[test]
    fn sourced_string_falls_back_to_cli() {
        let env = env_from(&[]);
        let value = sourced_string(
            &env,
            "trace".to_owned(),
            env_keys::LOG_LEVEL,
            None,
            "log.level",
            Some("info"),
            Some("--log-level"),
        );
        assert_eq!(value.source, ConfigSource::Cli);
        assert_eq!(value.source_detail.as_deref(), Some("--log-level"));
    }

    #[test]
    fn sourced_option_attributes_layers() {
        let env = env_from(&[(env_keys::NODE_LISTEN_ADDR, "127.0.0.1:1111")]);
        let value = sourced_option(
            &env,
            Some("127.0.0.1:1111".to_owned()),
            env_keys::NODE_LISTEN_ADDR,
            Some("127.0.0.1:2222".to_owned()),
            "node.listen_addr",
            Some("--listen"),
        );
        assert_eq!(value.source, ConfigSource::Env);

        let env = env_from(&[]);
        let value = sourced_option(
            &env,
            Some("127.0.0.1:2222".to_owned()),
            env_keys::NODE_LISTEN_ADDR,
            Some("127.0.0.1:2222".to_owned()),
            "node.listen_addr",
            Some("--listen"),
        );
        assert_eq!(value.source, ConfigSource::ConfigFile);

        let value = sourced_option(
            &env,
            Some("127.0.0.1:3333".to_owned()),
            env_keys::NODE_LISTEN_ADDR,
            Some("127.0.0.1:2222".to_owned()),
            "node.listen_addr",
            Some("--listen"),
        );
        assert_eq!(value.source, ConfigSource::Cli);
        assert_eq!(value.source_detail.as_deref(), Some("--listen"));

        let value = sourced_option(
            &env,
            None,
            env_keys::NODE_LISTEN_ADDR,
            Some("127.0.0.1:2222".to_owned()),
            "node.listen_addr",
            Some("--listen"),
        );
        assert_eq!(value.source, ConfigSource::Default);
        assert_eq!(value.value, None);
        assert_eq!(value.source_detail, None);
    }

    #[test]
    fn sourced_list_attributes_layers() {
        let env = env_from(&[(env_keys::NODE_ALLOWED_HOSTS, "a.test, b.test")]);
        let value = sourced_list(
            &env,
            vec!["a.test".to_owned(), "b.test".to_owned()],
            env_keys::NODE_ALLOWED_HOSTS,
            &["a.test".to_owned(), "b.test".to_owned()],
            "node.allowed_hosts",
        );
        assert_eq!(value.source, ConfigSource::Env);
        assert_eq!(
            value.source_detail.as_deref(),
            Some(env_keys::NODE_ALLOWED_HOSTS)
        );

        let env = env_from(&[]);
        let value = sourced_list(
            &env,
            vec!["a.test".to_owned()],
            env_keys::NODE_ALLOWED_HOSTS,
            &["a.test".to_owned()],
            "node.allowed_hosts",
        );
        assert_eq!(value.source, ConfigSource::ConfigFile);
        assert_eq!(value.source_detail.as_deref(), Some("node.allowed_hosts"));

        let value = sourced_list(
            &env,
            Vec::new(),
            env_keys::NODE_ALLOWED_HOSTS,
            &[],
            "node.allowed_hosts",
        );
        assert_eq!(value.source, ConfigSource::Default);
        assert!(value.value.is_empty());
        assert_eq!(value.source_detail, None);
    }

    #[test]
    fn agent_capabilities_default_to_subagents_on() {
        assert!(AgentAcpCapabilities::default().subagents);

        // An older snapshot without the field still decodes with the default.
        let decoded: AgentAcpCapabilities =
            serde_json::from_str("{}").expect("decode empty capabilities");
        assert!(decoded.subagents);

        let custom: CustomAgentConfig = serde_json::from_str(
            r#"{
                "command": "opencode",
                "args": [],
                "env": {},
                "env_secrets": {}
            }"#,
        )
        .expect("decode custom agent");
        assert!(custom.capabilities.subagents);
    }
}
