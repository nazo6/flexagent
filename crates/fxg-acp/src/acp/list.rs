//! Discovery of sessions that already exist on the agent side.
//!
//! ACP exposes `session/list` as a stable v1 request. Discovery only needs an
//! initialized connection (no `session/new`), so this module starts a short-lived
//! process, asks for the list, and drops the connection again.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §4.5.

use super::*;
use agent_client_protocol::v1::schema::{ListSessionsRequest, SessionInfo};
use std::path::Path;

/// Upper bound on paginated `session/list` requests per agent.
///
/// Discovery is a manual, interactive operation. A bounded number of pages keeps
/// a misbehaving cursor from looping forever; the node surfaces truncation.
const MAX_PAGES: usize = 10;

/// Why discovery failed.
#[derive(Debug)]
enum ListFailure {
    /// The agent does not advertise `sessionCapabilities.list`.
    Unsupported,
    /// The agent requires authentication first.
    Auth(Vec<AgentAuthMethodInfo>),
    /// Any other transport / protocol failure.
    Message(String),
}

impl ListFailure {
    fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Unsupported => crate::driver::ExternalSessionsUnsupported.into(),
            Self::Auth(methods) => AgentAuthRequired(methods).into(),
            Self::Message(message) => anyhow!(message),
        }
    }
}

/// Lists the sessions an agent already owns.
///
/// `process_cwd` is only used to resolve the executable (`PATH` / `PATHEXT`);
/// `session/list` is requested without a working-directory filter so the
/// caller can see sessions from every project.
pub(super) async fn list_sessions(
    launch: &crate::driver::AgentLaunchSpec,
    process_cwd: &Path,
) -> anyhow::Result<crate::driver::AgentSessionList> {
    let program = launch
        .program
        .as_deref()
        .ok_or_else(|| anyhow!("agent {} has no executable configured", launch.agent_id))?;
    let program =
        fxg_pty::resolve_command(&program.to_string_lossy(), process_cwd).map_err(|err| {
            anyhow!(
                "failed to resolve agent program {}: {err}",
                program.display()
            )
        })?;
    let config = AcpAgentConfig::new(program)
        .args(launch.args.iter().cloned())
        .envs(launch.env.iter().cloned());
    let acp = launch.acp.clone();

    let (tx, rx) = oneshot::channel();
    let connected = Client
        .v1()
        .connect_with(AcpAgent::new(config), async move |cx| {
            let outcome = query_sessions(&cx, &acp).await;
            fxg_protocol::discard_with!(tx.send(outcome), "return the session list to the caller");
            Ok(())
        })
        .await;

    match connected {
        Ok(()) => rx
            .await
            .map_err(|err| anyhow!("agent process ended before returning sessions: {err}"))?
            .map_err(ListFailure::into_anyhow),
        Err(err) => Err(anyhow!("failed to start the agent: {err}")),
    }
}

/// Runs `initialize` + paginated `session/list` on an initialized connection.
async fn query_sessions(
    cx: &ConnectionTo<Agent>,
    acp: &crate::agent::extension::AcpExtensionOptions,
) -> Result<crate::driver::AgentSessionList, ListFailure> {
    let initialize = cx
        .send_request(
            InitializeRequest::new(ProtocolVersion::V1)
                .client_capabilities(client_capabilities(acp))
                .client_info(Implementation::new("fxg", env!("CARGO_PKG_VERSION"))),
        )
        .block_task()
        .await
        .map_err(|err| ListFailure::Message(format!("acp initialize failed: {err}")))?;

    let auth_methods = initialize
        .auth_methods
        .iter()
        .map(map_auth_method)
        .collect::<Vec<_>>();
    let capabilities = &initialize.agent_capabilities;
    if capabilities.session_capabilities.list.is_none() {
        return Err(ListFailure::Unsupported);
    }
    let supports_history = capabilities.load_session;

    let mut sessions = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut request = ListSessionsRequest::new();
        if let Some(next) = cursor.take() {
            request = request.cursor(next);
        }
        let response = cx.send_request(request).block_task().await.map_err(|err| {
            if err.code == agent_client_protocol::ErrorCode::AUTH_REQUIRED {
                ListFailure::Auth(auth_methods.clone())
            } else {
                ListFailure::Message(format!("session/list failed: {err}"))
            }
        })?;
        sessions.extend(response.sessions.into_iter().map(map_session_info));
        match response.next_cursor {
            Some(next) if !next.is_empty() => cursor = Some(next),
            _ => {
                return Ok(crate::driver::AgentSessionList {
                    sessions,
                    supports_history,
                });
            }
        }
    }

    tracing::debug!("session/list pagination stopped at the page cap");
    Ok(crate::driver::AgentSessionList {
        sessions,
        supports_history,
    })
}

fn map_session_info(info: SessionInfo) -> crate::driver::AgentSessionInfo {
    crate::driver::AgentSessionInfo {
        session_id: info.session_id.to_string(),
        cwd: info.cwd,
        title: info.title,
        updated_at: info.updated_at,
    }
}
