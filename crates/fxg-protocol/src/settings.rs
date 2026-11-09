//! Typed settings, secret metadata and project identity types.
//!
//! Design: `docs/changelog/2026-11-09-settings-consolidation-and-project-identity.md`.
//!
//! Settings are never sent as a generic key/value container. Each owner (node,
//! server, bootstrap file, browser) exposes its own typed snapshot, and the Web UI
//! renders a searchable catalog over those values. Secret values never appear in
//! these types: only metadata (`SecretSummary`) is exposed, and writes go through
//! a write-only request.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::{ProjectId, SecretId};

crate::open_string_enum! {
    /// Where a bootstrap (process start-up) value came from.
    pub enum ConfigSource {
        /// Built-in default
        Default => "default",
        /// `~/.flexagent/config.toml`
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
    /// Environment variable name / config.toml key / CLI flag (when known)
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

/// Read-only bootstrap view of a local node (`config.toml` / env / CLI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct NodeBootstrapView {
    /// Data directory (`FXG_HOME`)
    pub fxg_home: SourcedValue<String>,
    /// Node ID
    pub node_id: SourcedValue<String>,
    /// Local HTTP/WS bind address
    pub listen_addr: SourcedValue<String>,
    /// Node Hub WebSocket URL (standalone when `None`)
    pub central_server_url: SourcedValue<Option<String>>,
    /// Log level
    pub log_level: SourcedValue<String>,
    /// Log file destination (empty when disabled)
    pub log_file: SourcedValue<Option<String>>,
    /// Log format (`text` / `json`)
    pub log_format: SourcedValue<String>,
    /// Whether ANSI colors are disabled
    pub log_no_color: SourcedValue<bool>,
}

/// Read-only bootstrap view of the central server (`config.toml` / env / CLI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ServerBootstrapView {
    /// Data directory (`FXG_HOME`)
    pub fxg_home: SourcedValue<String>,
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
    /// Whether remote (through the central server) Web PTY sessions are allowed
    pub allow_remote_pty: bool,
    /// Directories scanned by `fxg project scan` when no directory is given
    pub project_scan_dirs: Vec<String>,
    /// Tray residency on start-up (`None` = default: disabled)
    pub tray: Option<bool>,
    /// Optimistic-lock revision
    pub revision: u64,
    /// Last update time (Unix epoch ms)
    pub updated_at: i64,
}

/// `PATCH /api/v1/settings/node` request.
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
    /// Revision the client observed (`REVISION_MISMATCH` on conflict)
    pub expected_revision: u64,
}

/// Node-owned agent settings stored in `node.db` (`agent_settings` / `agent_aliases`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentSettings {
    /// Default agent ID (`None` = built-in default)
    pub default_agent: Option<String>,
    /// ACP Registry URL (`None` = built-in default)
    pub registry_url: Option<String>,
    /// Registry cache TTL in seconds (`None` = built-in default)
    pub registry_cache_ttl_secs: Option<u64>,
    /// Agent name aliases (merged over the built-in defaults)
    pub aliases: BTreeMap<String, String>,
    /// Optimistic-lock revision
    pub revision: u64,
    /// Last update time (Unix epoch ms)
    pub updated_at: i64,
}

impl AgentSettings {
    /// Default value of the default agent.
    pub const DEFAULT_AGENT: &'static str = "opencode";
    /// Default value of the official ACP registry URL.
    pub const DEFAULT_REGISTRY_URL: &'static str =
        "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
    /// Default value of the registry cache TTL (24 hours).
    pub const DEFAULT_REGISTRY_CACHE_TTL_SECS: u64 = 86_400;

    /// Built-in default aliases.
    pub fn default_aliases() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("claude".to_owned(), "claude-code-acp".to_owned()),
            ("antigravity".to_owned(), "agy".to_owned()),
            ("gemini".to_owned(), "gemini-cli-acp".to_owned()),
            ("codex".to_owned(), "codex-acp".to_owned()),
        ])
    }

    /// Returns the effective aliases: built-ins merged with the configured ones.
    pub fn resolved_aliases(&self) -> BTreeMap<String, String> {
        let mut aliases = Self::default_aliases();
        for (key, value) in &self.aliases {
            aliases.insert(key.clone(), value.clone());
        }
        aliases
    }

    /// Returns the effective default agent.
    pub fn resolved_default_agent(&self) -> &str {
        self.default_agent
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(Self::DEFAULT_AGENT)
    }

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

/// `PATCH /api/v1/settings/agents` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UpdateAgentSettingsRequest {
    /// Default agent ID
    pub default_agent: Option<String>,
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
}

/// Server-owned settings stored in `server.db` (`server_settings`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ServerSettings {
    /// Web Push contact claim (`mailto:` / `https:`)
    pub vapid_subject: Option<String>,
    /// Per-host Git credential provider
    pub git_credentials: BTreeMap<String, GitCredentialConfig>,
    /// Optimistic-lock revision
    pub revision: u64,
    /// Last update time (Unix epoch ms)
    pub updated_at: i64,
}

/// `PATCH /api/v1/settings/server` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UpdateServerSettingsRequest {
    /// Web Push contact claim
    pub vapid_subject: Option<String>,
    /// Per-host Git credential provider
    pub git_credentials: BTreeMap<String, GitCredentialConfig>,
    /// Revision the client observed (`REVISION_MISMATCH` on conflict)
    pub expected_revision: u64,
}

/// Complete settings snapshot of the central server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ServerSettingsSnapshot {
    /// Read-only process bootstrap values
    pub bootstrap: ServerBootstrapView,
    /// Server-owned settings
    pub server: ServerSettings,
}

/// How a temporary VM obtains Git credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "provider", rename_all = "snake_case")]
#[specta(collect)]
pub enum GitCredentialConfig {
    /// Obtain the token from `gh auth token`.
    GhCli {
        /// Basic authentication user name (default: `x-access-token`)
        #[serde(default)]
        username: Option<String>,
    },
    /// Obtain the token from the given environment variable.
    Env {
        /// Basic authentication user name (default: `x-access-token`)
        #[serde(default)]
        username: Option<String>,
        /// Name of the environment variable holding the token
        token_env: String,
    },
    /// Read the token from the secret store (`secrets/server/<id>`).
    Secret {
        /// Basic authentication user name (default: `x-access-token`)
        #[serde(default)]
        username: Option<String>,
        /// Referenced secret
        secret_id: SecretId,
    },
}

impl GitCredentialConfig {
    /// Effective Basic authentication user name.
    pub fn resolved_username(&self) -> &str {
        let configured = match self {
            Self::GhCli { username }
            | Self::Env { username, .. }
            | Self::Secret { username, .. } => username.as_deref(),
        };
        configured
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(crate::config::DEFAULT_GIT_USERNAME)
    }
}

/// A custom ACP agent definition (`agent_custom` in `node.db`).
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
}

/// Definition of an ephemeral VM / sandbox provisioner (`provisioners` in `server.db`).
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
    /// Placeholders: `{BOOTSTRAP_SCRIPT}` / `{INSTANCE_NAME}` / `{SESSION_ID}`.
    pub args: Vec<String>,
    /// Seconds until the idle instance is discarded automatically
    pub idle_timeout_secs: Option<u64>,
    /// Environment variables injected into the process
    pub env: BTreeMap<String, String>,
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

/// `PUT /api/v1/secrets/{secret_id}` request (write-only).
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

/// Per-project bootstrap profile for ephemeral VMs (`project_profiles` in `server.db`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectProfile {
    /// Logical project ID
    pub project_id: ProjectId,
    /// Setup commands run after `git clone` and tool detection
    pub bootstrap_setup: Vec<String>,
    /// Non-secret environment variables injected during setup and execution
    pub bootstrap_env: BTreeMap<String, String>,
    /// Optimistic-lock revision
    pub revision: u64,
    /// Last update time (Unix epoch ms)
    pub updated_at: i64,
}

/// `PUT /api/v1/projects/{project_id}/profile` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UpdateProjectProfileRequest {
    /// Setup commands
    pub bootstrap_setup: Vec<String>,
    /// Non-secret environment variables
    pub bootstrap_env: BTreeMap<String, String>,
    /// Revision the client observed (`REVISION_MISMATCH` on conflict)
    pub expected_revision: u64,
}

/// Response of the secret list endpoints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SecretsResponse {
    /// Secret metadata (values are never returned)
    pub secrets: Vec<SecretSummary>,
}
