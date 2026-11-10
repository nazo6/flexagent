//! CLI subcommands (argument definitions and execution).
//!
//! The `usage` derives are the single source of truth; the KDL spec, shell
//! completions, manpage and Markdown reference are generated from
//! `__usage_spec__` (design: `docs/05-cli-and-pwa-ui.md` §1).
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use fxg_acp::registry::AcpRegistry;
use fxg_protocol::common::AgentAuthMethodKind;
use usage::Args;

use crate::client::DaemonClient;

use super::node_db;

// ----------------------------------------------------------------------
// fxg internal <command>
// ----------------------------------------------------------------------

/// Arguments of `fxg internal <command>`.
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct InternalArgs {
    #[usage(subcommand)]
    command: InternalCommands,
}

/// Internal commands invoked by Git, provisioners, or system integrations.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum InternalCommands {
    /// GIT_ASKPASS helper for the Git credential proxy
    GitAskpass(GitAskpassArgs),
    /// Zero-touch workspace bootstrap inside ephemeral VMs
    BootstrapWorkspace(BootstrapWorkspaceArgs),
}

// ----------------------------------------------------------------------
// fxg internal bootstrap-workspace
// ----------------------------------------------------------------------

/// Arguments of `fxg internal bootstrap-workspace` (run by the provisioner inside an
/// ephemeral VM).
#[derive(Debug, Args)]
pub struct BootstrapWorkspaceArgs {
    /// Git URL to clone from
    #[usage(long)]
    repo: String,
    /// Target branch (defaults to the remote's default branch)
    #[usage(long)]
    branch: Option<String>,
    /// Destination directory (default: /tmp/workspace)
    #[usage(long)]
    dir: Option<String>,
    /// Project profile JSON (setup commands and env) written by the server
    #[usage(long)]
    profile: Option<String>,
}

impl usage::RunAsync for BootstrapWorkspaceArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let dir = self
            .dir
            .clone()
            .unwrap_or_else(|| "/tmp/workspace".to_owned());
        let profile = match &self.profile {
            Some(path) => Some(
                serde_json::from_str::<fxg_protocol::settings::ProjectProfile>(
                    &std::fs::read_to_string(path)
                        .with_context(|| format!("failed to read profile {path}"))?,
                )
                .with_context(|| format!("failed to parse project profile {path}"))?,
            ),
            None => match env("FXG_PROJECT_PROFILE_B64") {
                Some(encoded) => {
                    use base64::Engine as _;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(encoded.trim().as_bytes())
                        .context("failed to decode FXG_PROJECT_PROFILE_B64")?;
                    Some(
                        serde_json::from_slice::<fxg_protocol::settings::ProjectProfile>(&bytes)
                            .context("failed to parse FXG_PROJECT_PROFILE_B64")?,
                    )
                }
                None => None,
            },
        };
        let (setup, profile_env) = profile
            .map(|profile| (profile.bootstrap_setup, profile.bootstrap_env))
            .unwrap_or_default();
        let options = fxg_node::bootstrap::BootstrapOptions {
            repo: self.repo.clone(),
            branch: self.branch.clone(),
            dir: PathBuf::from(&dir),
            fxg_home: fxg_protocol::config::fxg_home(&env),
            install_toolchain: true,
            extra_tools: Vec::new(),
            setup,
            env: profile_env,
        };
        let outcome = fxg_node::bootstrap::bootstrap_workspace(options)
            .await
            .context("bootstrap-workspace failed")?;
        // stdout is reserved for the protocol, so the result goes to stderr too
        eprintln!(
            "[fxg-bootstrap] ready: {} (tools: {})",
            outcome.dir.display(),
            outcome.tools.join(", ")
        );
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg internal git-askpass (GIT_ASKPASS helper)
// ----------------------------------------------------------------------

/// Arguments of `fxg internal git-askpass` (an internal command Git invokes as
/// `GIT_ASKPASS`).
#[derive(Debug, Args)]
pub struct GitAskpassArgs {
    /// Prompt string passed by Git (`Username for '...'` / `Password for '...'`)
    prompt: String,
}

impl usage::RunAsync for GitAskpassArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let (username, token, error) = client.git_credential(&self.prompt).await?;
        if let Some(error) = error {
            bail!("{error}");
        }
        let value = if fxg_node::credentials::prompt_wants_username(&self.prompt) {
            username
        } else {
            token.context("no credential token is available for this prompt")?
        };
        println!("{value}");
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg agents <command>
// ----------------------------------------------------------------------

/// `fxg agents <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct AgentsArgs {
    #[usage(subcommand)]
    command: AgentsCommands,
}

/// ACP agent authentication subcommands.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum AgentsCommands {
    /// Log in to an agent using its advertised ACP authentication methods
    Login(AgentsLoginArgs),
    /// Log out from an agent (when the agent supports it)
    Logout(AgentsLogoutArgs),
}

/// Builds the ACP Registry from the node-owned agent settings.
async fn acp_registry() -> Result<AcpRegistry> {
    let env = fxg_protocol::config::process_env;
    let paths = fxg_node::NodePaths::from_env(&env);
    let db = node_db(&env).await?;
    let settings = db.ensure_agent_settings().await?;
    Ok(AcpRegistry::new(
        paths.fxg_home().to_path_buf(),
        &settings,
        db.custom_agents().await?,
        db.enabled_package_agents().await?,
    ))
}

/// `fxg agents login <id>`
#[derive(Debug, Args)]
pub struct AgentsLoginArgs {
    /// Agent ID (aliases allowed)
    id: String,
    /// Authentication method ID (omitted: prefer a terminal method)
    #[usage(long)]
    method: Option<String>,
}

/// Resolves a launch spec for `agents login/logout`.
async fn resolve_agent_launch_spec(
    registry: &AcpRegistry,
    id: &str,
) -> Result<fxg_acp::AgentLaunchSpec> {
    // Custom definitions and presets are resolved before the registry index,
    // so an offline/missing registry still allows logging in to a local CLI.
    let index = match registry.index(false).await {
        Ok(index) => index,
        Err(err) => {
            tracing::warn!(agent_id = id, "failed to load ACP registry: {err}");
            fxg_acp::registry::RegistryIndex::default()
        }
    };
    registry
        .launch_spec(id, &index, &[])
        .await
        .with_context(|| format!("cannot launch {id}"))
}

/// Selects an authentication method (explicit ID, terminal preference, or the
/// only advertised method).
pub(super) fn choose_auth_method(
    methods: &[fxg_protocol::common::AgentAuthMethodInfo],
    requested: Option<&str>,
) -> Result<fxg_protocol::common::AgentAuthMethodInfo> {
    if let Some(wanted) = requested {
        return methods
            .iter()
            .find(|method| method.id == wanted)
            .cloned()
            .with_context(|| {
                format!(
                    "unknown authentication method {wanted}; available: {}",
                    methods
                        .iter()
                        .map(|method| method.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
    }
    if let Some(terminal) = methods
        .iter()
        .find(|method| method.kind == AgentAuthMethodKind::Terminal)
        .cloned()
    {
        return Ok(terminal);
    }
    if methods.len() == 1 {
        return Ok(methods[0].clone());
    }
    for method in methods {
        println!("  {} ({}) - {}", method.id, method.kind, method.name);
    }
    bail!("multiple authentication methods; pass --method <id>")
}

impl usage::RunAsync for AgentsLoginArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let registry = acp_registry().await?;
        let id = registry.resolve_alias(&self.id);
        let spec = resolve_agent_launch_spec(&registry, &id).await?;
        let methods = fxg_acp::auth::discover_auth_methods(&spec)
            .await
            .with_context(|| format!("failed to query authentication methods for {id}"))?;
        if methods.is_empty() {
            println!("{id} does not advertise authentication methods; it may already be signed in");
            return Ok(());
        }
        let method = choose_auth_method(&methods, self.method.as_deref())?;
        println!("using {} ({})", method.name, method.id);
        match method.kind {
            AgentAuthMethodKind::Terminal => {
                fxg_acp::auth::run_terminal_login(&spec, &method).await?;
            }
            AgentAuthMethodKind::Agent => {
                fxg_acp::auth::authenticate(&spec, &method.id).await?;
            }
            _ => bail!("unsupported authentication method kind: {}", method.kind),
        }
        println!("logged in to {id}");
        Ok(())
    }
}

/// `fxg agents logout <id>`
#[derive(Debug, Args)]
pub struct AgentsLogoutArgs {
    /// Agent ID (aliases allowed)
    id: String,
}

impl usage::RunAsync for AgentsLogoutArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let registry = acp_registry().await?;
        let id = registry.resolve_alias(&self.id);
        let spec = resolve_agent_launch_spec(&registry, &id).await?;
        fxg_acp::auth::logout(&spec)
            .await
            .with_context(|| format!("failed to log out from {id}"))?;
        println!("logged out of {id}");
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg auth <command>
// ----------------------------------------------------------------------

/// `fxg auth <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct AuthArgs {
    #[usage(subcommand)]
    command: AuthCommands,
}

/// Auth token management subcommands.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum AuthCommands {
    /// Show the client auth token (auth_token)
    Token(AuthTokenArgs),
    /// Regenerate the client auth token
    RotateToken(AuthRotateArgs),
    /// Manage per-node tokens (Node ⇔ Server pairing)
    #[usage(name = "node-token")]
    NodeToken(NodeTokenArgs),
}

/// Subcommands of `fxg auth node-token`.
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct NodeTokenArgs {
    #[usage(subcommand)]
    command: NodeTokenCommands,
}

/// Per-node token operations.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum NodeTokenCommands {
    /// Issue and print a per-node token (run on the central server)
    Issue(NodeTokenIssueArgs),
    /// Revoke a per-node token
    Revoke(NodeTokenRevokeArgs),
    /// List issued node tokens
    List(NodeTokenListArgs),
}

/// `fxg auth node-token issue <node-id>`
#[derive(Debug, Args)]
pub struct NodeTokenIssueArgs {
    /// Target node ID (the `fxg daemon` node_id)
    node_id: String,
}

impl usage::RunAsync for NodeTokenIssueArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let db = open_hub_db().await?;
        // Register unknown nodes with placeholder values
        // (overwritten with real values by `NodeHello` when the node connects)
        let nodes = db.list_nodes().await?;
        if !nodes
            .iter()
            .any(|node| node.node_id.as_str() == self.node_id)
        {
            db.upsert_node(&fxg_db::NodeRecord::new(
                self.node_id.clone(),
                self.node_id.clone(),
                "unknown",
                "unknown",
                "unknown",
            ))
            .await?;
        }
        let token = fxg_server::api::auth::generate_token();
        db.set_node_token_hash(
            &self.node_id,
            Some(&fxg_server::api::auth::hash_token(&token)),
        )
        .await?;
        // The plaintext token is shown only once at issuance (the server stores
        // only the hash)
        println!("{token}");
        eprintln!("issued a node_token; save it to ~/.flexagent/node_token on the node side.");
        Ok(())
    }
}

/// `fxg auth node-token revoke <node-id>`
#[derive(Debug, Args)]
pub struct NodeTokenRevokeArgs {
    /// Target node ID
    node_id: String,
}

impl usage::RunAsync for NodeTokenRevokeArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let db = open_hub_db().await?;
        db.set_node_token_hash(&self.node_id, None).await?;
        println!("revoked node token for {}", self.node_id);
        Ok(())
    }
}

/// `fxg auth node-token list`
#[derive(Debug, Args)]
pub struct NodeTokenListArgs {}

impl usage::RunAsync for NodeTokenListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let db = open_hub_db().await?;
        let tokens = db.list_node_tokens().await?;
        if tokens.is_empty() {
            println!("(no node tokens issued)");
            return Ok(());
        }
        for (node_id, hash) in tokens {
            let prefix: String = hash.chars().take(12).collect();
            println!("{node_id}\t{prefix}…");
        }
        Ok(())
    }
}

/// Open the central server DB (`~/.flexagent/server.db`).
async fn open_hub_db() -> Result<fxg_db::Db> {
    let env = fxg_protocol::config::process_env;
    let home = fxg_protocol::config::fxg_home(&env);
    let db = fxg_db::Db::open(&fxg_db::hub_db_path(&home), fxg_db::DbRole::Hub).await?;
    Ok(db)
}

/// `fxg auth token`
#[derive(Debug, Args)]
pub struct AuthTokenArgs {}

impl usage::RunAsync for AuthTokenArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        println!("{}", client.auth_token(false).await?);
        Ok(())
    }
}

/// `fxg auth rotate-token`
#[derive(Debug, Args)]
pub struct AuthRotateArgs {}

impl usage::RunAsync for AuthRotateArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let token = client.auth_token(true).await?;
        println!("{token}");
        eprintln!("regenerated the auth token (browsers and other clients must enter it again)");
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg kill-all
// ----------------------------------------------------------------------

/// Arguments of `fxg kill-all`.
#[derive(Debug, Args)]
pub struct KillAllArgs {
    /// Stop only the local node without fanning out through the central server
    /// (stopping every node at once is fanned out by the central server's
    /// `POST /api/v1/system/kill-switch`)
    #[usage(long)]
    local_only: bool,
}

impl usage::RunAsync for KillAllArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let (sessions, ptys) = client.kill_all().await?;
        if !self.local_only {
            tracing::debug!(
                "all-node fan-out is issued by the central server via POST /api/v1/system/kill-switch"
            );
        }
        println!(
            "killed {} session(s) and {} pty(s)",
            sessions.len(),
            ptys.len()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::common::{AgentAuthMethodInfo, AgentAuthMethodKind};
    use std::collections::BTreeMap;

    fn auth_method(id: &str, kind: AgentAuthMethodKind) -> AgentAuthMethodInfo {
        AgentAuthMethodInfo {
            id: id.to_owned(),
            name: id.to_owned(),
            description: None,
            kind,
            args: Vec::new(),
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn choose_auth_method_prefers_terminal() {
        let methods = vec![
            auth_method("agent-login", AgentAuthMethodKind::Agent),
            auth_method("terminal-login", AgentAuthMethodKind::Terminal),
        ];
        let chosen = choose_auth_method(&methods, None).expect("terminal method");
        assert_eq!(chosen.id, "terminal-login");
    }

    #[test]
    fn choose_auth_method_honors_explicit_id() {
        let methods = vec![
            auth_method("agent-login", AgentAuthMethodKind::Agent),
            auth_method("terminal-login", AgentAuthMethodKind::Terminal),
        ];
        let chosen = choose_auth_method(&methods, Some("agent-login")).expect("explicit method");
        assert_eq!(chosen.id, "agent-login");
    }

    #[test]
    fn choose_auth_method_requires_a_choice_when_ambiguous() {
        let methods = vec![
            auth_method("a", AgentAuthMethodKind::Agent),
            auth_method("b", AgentAuthMethodKind::Agent),
        ];
        let err = choose_auth_method(&methods, None).expect_err("ambiguous");
        assert!(err.to_string().contains("--method"), "got: {err}");
    }
}
