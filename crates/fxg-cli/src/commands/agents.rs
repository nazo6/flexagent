//! `fxg agents` commands (ACP agent authentication).

use anyhow::{Context, Result, bail};
use fxg_acp::agent::AgentCatalog;
use fxg_acp::registry::AcpRegistry;
use fxg_protocol::common::AgentAuthMethodKind;
use usage::{Args, Subcommands};

use super::node_db;

/// `fxg agents <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct AgentsArgs {
    #[usage(subcommand)]
    command: AgentsCommands,
}

/// ACP agent authentication subcommands.
#[derive(Debug, Subcommands)]
#[usage(run_async)]
pub enum AgentsCommands {
    /// Log in to an agent using its advertised ACP authentication methods
    Login(AgentsLoginArgs),
    /// Log out from an agent (when the agent supports it)
    Logout(AgentsLogoutArgs),
}

/// Builds the agent catalog from the node-owned settings.
async fn agent_catalog() -> Result<AgentCatalog> {
    let env = fxg_protocol::config::process_env;
    let paths = fxg_node::NodePaths::from_env(&env);
    let db = node_db(&env).await?;
    let settings = db.ensure_agent_settings().await?;
    Ok(AgentCatalog::new(AcpRegistry::new(
        paths.fxg_home().to_path_buf(),
        &settings,
        db.custom_agents().await?,
        db.enabled_package_agents().await?,
    )))
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
    catalog: &AgentCatalog,
    id: &str,
) -> Result<fxg_acp::AgentLaunchSpec> {
    // Custom definitions and presets are resolved before the registry index,
    // so an offline/missing registry still allows logging in to a local CLI.
    let index = match catalog.index(false).await {
        Ok(index) => index,
        Err(err) => {
            tracing::warn!(agent_id = id, "failed to load ACP registry: {err}");
            fxg_acp::registry::RegistryIndex::default()
        }
    };
    catalog
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
        let catalog = agent_catalog().await?;
        let id = catalog.resolve_alias(&self.id);
        let spec = resolve_agent_launch_spec(&catalog, &id).await?;
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
        let catalog = agent_catalog().await?;
        let id = catalog.resolve_alias(&self.id);
        let spec = resolve_agent_launch_spec(&catalog, &id).await?;
        fxg_acp::auth::logout(&spec)
            .await
            .with_context(|| format!("failed to log out from {id}"))?;
        println!("logged out of {id}");
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
