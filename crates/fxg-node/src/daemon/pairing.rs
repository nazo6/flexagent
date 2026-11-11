//! Node ⇔ hub pairing (owned by `node.db` and `~/.flexagent/node_token`).
//!
//! Design: `docs/changelog/2026-11-11-node-pairing-db-and-gui.md` §4.
//!
//! - The hub address is stored in `node.db` (`node_pairing`), so it is a
//!   runtime setting like every other node-owned value.
//! - The token is a secret and stays in the fixed `~/.flexagent/node_token`
//!   (`0600`); the client API is write-only and never returns it.
//! - Saving or clearing the pairing restarts the sync supervisor immediately
//!   (no `fxg node restart`).

use fxg_protocol::SecretId;
use fxg_protocol::client_api::{HubLinkState, HubLinkStatus};
use fxg_protocol::settings::{PairingSettings, TestPairingRequest, UpdatePairingRequest};

use super::DaemonState;
use super::ops::AuditSource;
use crate::error::NodeError;
use crate::sync::{self, HubTarget};

impl DaemonState {
    /// Re-reads the pairing and reconfigures the sync supervisor.
    pub async fn refresh_hub_target(&self) -> Result<Option<HubTarget>, NodeError> {
        let pairing = self.db().ensure_node_pairing().await?;
        let Some(url) = pairing.hub_url else {
            self.set_hub_target(None);
            self.set_hub_link(HubLinkStatus::standalone());
            return Ok(None);
        };

        let hub_url = Some(url.clone());
        let Some(token) = self.pairing_token() else {
            self.set_hub_target(None);
            self.set_hub_link(HubLinkStatus::new(
                HubLinkState::Failed,
                hub_url,
                Some("the pairing token is missing; pair the node again".to_owned()),
            ));
            return Ok(None);
        };

        let target = HubTarget {
            ws_url: fxg_protocol::config::node_ws_url(&url)?,
            token,
        };
        self.set_hub_target(Some(target.clone()));
        Ok(Some(target))
    }

    /// Reads the pairing token file (never logged).
    fn pairing_token(&self) -> Option<String> {
        match std::fs::read_to_string(self.paths().node_token_path()) {
            Ok(token) if !token.trim().is_empty() => Some(token.trim().to_owned()),
            _ => None,
        }
    }

    /// Builds the pairing snapshot (`GET /api/v1/nodes/{node_id}/settings`).
    pub async fn pairing_settings(&self) -> Result<PairingSettings, NodeError> {
        let pairing = self.db().ensure_node_pairing().await?;
        Ok(PairingSettings {
            hub_url: pairing.hub_url,
            token_configured: self.pairing_token().is_some(),
            revision: pairing.revision,
            connection: self.hub_link(),
            sessions_truncated: self.sessions_truncated(),
        })
    }

    /// Saves the pairing (hub address and/or token) and applies it immediately.
    pub async fn update_pairing(
        &self,
        request: &UpdatePairingRequest,
        source: &AuditSource,
    ) -> Result<PairingSettings, NodeError> {
        let current = self.db().ensure_node_pairing().await?;
        let ws_url = fxg_protocol::config::node_ws_url(&request.hub_url)?;
        let origin = fxg_protocol::config::hub_http_origin(&ws_url);
        let url_changed = current.hub_url.as_deref() != Some(origin.as_str());
        let token = request
            .token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());

        // Never send an existing token to a different hub.
        if token.is_none() && url_changed {
            return Err(NodeError::PairingTokenRequired);
        }

        // Persist the address first: a stale revision must not overwrite the token.
        self.db()
            .set_node_pairing_hub_url(Some(&origin), request.expected_revision)
            .await?;
        if let Some(token) = token {
            let path = self.paths().node_token_path();
            super::save_token_file(&path, token)?;
        }
        self.register_pairing_secret().await;
        self.record_audit(
            fxg_db::audit::actions::PAIRING_UPDATE,
            source,
            None,
            serde_json::json!({
                "hub_url": origin,
                "token_changed": token.is_some(),
            }),
        )
        .await;

        self.refresh_hub_target().await?;
        self.pairing_settings().await
    }

    /// Removes the pairing (the outbox is kept) and stops syncing immediately.
    pub async fn clear_pairing(
        &self,
        expected_revision: u64,
        source: &AuditSource,
    ) -> Result<PairingSettings, NodeError> {
        self.db()
            .set_node_pairing_hub_url(None, expected_revision)
            .await?;
        let path = self.paths().node_token_path();
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(NodeError::Io { path, source }),
        }
        self.record_audit(
            fxg_db::audit::actions::PAIRING_UPDATE,
            source,
            None,
            serde_json::json!({ "hub_url": null }),
        )
        .await;

        self.refresh_hub_target().await?;
        self.pairing_settings().await
    }

    /// Tries a hub address / token without saving it.
    pub async fn test_pairing(
        &self,
        request: &TestPairingRequest,
    ) -> Result<HubLinkStatus, NodeError> {
        let ws_url = fxg_protocol::config::node_ws_url(&request.hub_url)?;
        let token = request
            .token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_owned)
            .or_else(|| self.pairing_token())
            .ok_or(NodeError::PairingTokenRequired)?;
        Ok(sync::test_connection(&ws_url, &token).await)
    }

    /// Registers the system secret metadata for the pairing token.
    pub async fn register_pairing_secret(&self) {
        let id = SecretId::new(fxg_db::secrets::SYSTEM_NODE_TOKEN);
        if let Err(err) =
            fxg_db::secrets::ensure_system_secret(self.db(), &id, "Node pairing token (system)")
                .await
        {
            tracing::warn!("failed to register the pairing token metadata: {err}");
        }
    }
}
