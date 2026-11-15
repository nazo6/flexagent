//! Native OpenCode authentication helpers.
//!
//! OpenCode has no ACP `initialize` handshake on the native path, so the
//! terminal login / logout commands are declared statically. They match the
//! terminal auth method that `opencode acp` advertises.

use std::collections::BTreeMap;

use fxg_protocol::common::{AgentAuthMethodInfo, AgentAuthMethodKind};

use crate::builtin::{AgentAuth, AgentCommand};

/// Authentication method ID shown by `fxg agents login`.
pub const AUTH_METHOD_ID: &str = "opencode-login";

/// Returns the native OpenCode authentication surface.
pub fn auth() -> AgentAuth {
    AgentAuth {
        methods: vec![AgentAuthMethodInfo {
            id: AUTH_METHOD_ID.to_owned(),
            name: "Login with OpenCode".to_owned(),
            description: Some("Run `opencode auth login` in the terminal".to_owned()),
            kind: AgentAuthMethodKind::Terminal,
            args: Vec::new(),
            env: BTreeMap::new(),
        }],
        login: Some(AgentCommand {
            program: "opencode",
            args: &["auth", "login"],
        }),
        logout: Some(AgentCommand {
            program: "opencode",
            args: &["auth", "logout"],
        }),
    }
}
