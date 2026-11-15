//! ACP authentication helpers (`initialize` / `authenticate` / `logout`).
//!
//! The ACP `initialize` response advertises authentication methods. Terminal
//! methods are executed as an interactive child process (the agent CLI) with
//! the method's extra arguments, while agent methods are forwarded as an
//! `authenticate` request on an initialized connection.

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    AuthMethodId, AuthenticateRequest, Implementation, InitializeRequest, LogoutRequest,
};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Client};
use anyhow::{Result, anyhow, bail};
use fxg_protocol::common::{AgentAuthMethodInfo, AgentAuthMethodKind};
use tokio::sync::oneshot;

use super::{client_capabilities, map_auth_method};
use crate::auth::run_interactive;
use crate::driver::AgentLaunchSpec;

/// Builds the spawn configuration from a resolved launch spec.
fn agent_config(spec: &AgentLaunchSpec) -> Result<AcpAgentConfig> {
    let program = spec
        .program
        .clone()
        .ok_or_else(|| anyhow!("agent {} has no executable configured", spec.agent_id))?;
    Ok(AcpAgentConfig::new(program)
        .args(spec.args.iter().cloned())
        .envs(spec.env.iter().cloned()))
}

/// Initializes a fresh agent process and returns its advertised auth methods.
pub async fn discover_auth_methods(spec: &AgentLaunchSpec) -> Result<Vec<AgentAuthMethodInfo>> {
    let (tx, rx) = oneshot::channel();
    let connected = Client
        .v1()
        .connect_with(AcpAgent::new(agent_config(spec)?), async move |cx| {
            let outcome = async {
                let initialize = cx
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(client_capabilities(
                                &crate::acp::extensions::AcpExtensionOptions::default(),
                            ))
                            .client_info(Implementation::new("fxg", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
                    .await
                    .map_err(|err| err.to_string())?;
                Ok::<_, String>(
                    initialize
                        .auth_methods
                        .iter()
                        .map(map_auth_method)
                        .collect::<Vec<_>>(),
                )
            }
            .await;
            fxg_protocol::discard_with!(tx.send(outcome), "return auth methods to the caller");
            Ok(())
        })
        .await;
    match connected {
        Ok(()) => rx
            .await
            .map_err(|err| anyhow!("agent process ended before returning auth methods: {err}"))?
            .map_err(|err| anyhow!("acp initialize failed: {err}")),
        Err(err) => Err(anyhow!("failed to start the agent: {err}")),
    }
}

/// Authenticates with an agent-managed method (`AuthMethod::Agent`).
pub async fn authenticate(spec: &AgentLaunchSpec, method_id: &str) -> Result<()> {
    let method_id = method_id.to_owned();
    let (tx, rx) = oneshot::channel();
    let connected = Client
        .v1()
        .connect_with(AcpAgent::new(agent_config(spec)?), async move |cx| {
            let outcome = async {
                let _initialize = cx
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(client_capabilities(
                                &crate::acp::extensions::AcpExtensionOptions::default(),
                            ))
                            .client_info(Implementation::new("fxg", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
                    .await
                    .map_err(|err| err.to_string())?;
                cx.send_request(AuthenticateRequest::new(AuthMethodId::new(method_id)))
                    .block_task()
                    .await
                    .map_err(|err| err.to_string())?;
                Ok::<_, String>(())
            }
            .await;
            fxg_protocol::discard_with!(tx.send(outcome), "return the auth result to the caller");
            Ok(())
        })
        .await;
    match connected {
        Ok(()) => rx
            .await
            .map_err(|err| anyhow!("agent process ended before authentication completed: {err}"))?
            .map_err(|err| anyhow!("authentication failed: {err}")),
        Err(err) => Err(anyhow!("failed to start the agent: {err}")),
    }
}

/// Logs out from an agent (`auth/logout`), when advertised.
pub async fn logout(spec: &AgentLaunchSpec) -> Result<()> {
    let (tx, rx) = oneshot::channel();
    let connected = Client
        .v1()
        .connect_with(AcpAgent::new(agent_config(spec)?), async move |cx| {
            let outcome = async {
                let initialize = cx
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(client_capabilities(
                                &crate::acp::extensions::AcpExtensionOptions::default(),
                            ))
                            .client_info(Implementation::new("fxg", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
                    .await
                    .map_err(|err| err.to_string())?;
                if initialize.agent_capabilities.auth.logout.is_none() {
                    return Err("the agent does not advertise logout support".to_owned());
                }
                cx.send_request(LogoutRequest::new())
                    .block_task()
                    .await
                    .map_err(|err| err.to_string())?;
                Ok::<_, String>(())
            }
            .await;
            fxg_protocol::discard_with!(tx.send(outcome), "return the logout result to the caller");
            Ok(())
        })
        .await;
    match connected {
        Ok(()) => rx
            .await
            .map_err(|err| anyhow!("agent process ended before logout completed: {err}"))?
            .map_err(|err| anyhow!("logout failed: {err}")),
        Err(err) => Err(anyhow!("failed to start the agent: {err}")),
    }
}

/// Runs a terminal authentication method interactively.
///
/// The ACP spec defines terminal methods as "the configured agent invocation
/// plus the method's arguments", so both the preset / registry args (`acp`) and
/// the method args (`--login`) are passed.
pub async fn run_terminal_login(
    spec: &AgentLaunchSpec,
    method: &AgentAuthMethodInfo,
) -> Result<()> {
    if method.kind != AgentAuthMethodKind::Terminal {
        bail!("{} is not a terminal authentication method", method.id);
    }
    let program_path = spec
        .program
        .as_deref()
        .ok_or_else(|| anyhow!("agent {} has no executable configured", spec.agent_id))?;
    let mut args = spec.args.clone();
    args.extend(method.args.iter().cloned());
    let mut env = spec.env.clone();
    env.extend(
        method
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );

    run_interactive(program_path, &args, &env).await
}
