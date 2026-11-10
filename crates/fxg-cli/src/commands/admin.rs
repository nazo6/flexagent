//! CLI subcommands (argument definitions and execution).
//!
//! The `usage` derives are the single source of truth; the KDL spec, shell
//! completions, manpage and Markdown reference are generated from
//! `__usage_spec__` (design: `docs/05-cli-and-pwa-ui.md` §1).
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use usage::Args;

use crate::client::DaemonClient;

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
            .context("fxg bootstrap-workspace failed")?;
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
