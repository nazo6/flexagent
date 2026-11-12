//! Server settings, secrets and project profiles.

use fxg_protocol::common::ErrorCode;

use crate::api::{ApiError, ClientInfo};
use crate::error::HubError;
use fxg_protocol::SecretId;
use fxg_protocol::client_api::{ProvisionerSummary, ProvisionersResponse};
use fxg_protocol::settings::{
    ConfigSource, HubBootstrapView, HubSettingsSnapshot, ProvisionerDefinition, SecretsResponse,
    SourcedValue, UpdateHubSettingsRequest, UpsertSecretRequest, sourced_option, sourced_string,
};

use super::HubState;

impl HubState {
    /// Builds the typed settings snapshot exposed by `GET /api/v1/settings/hub`.
    pub async fn hub_settings_snapshot(&self) -> Result<HubSettingsSnapshot, HubError> {
        let settings = self.db().ensure_hub_settings().await?;
        let config = match fxg_protocol::config::GlobalConfig::load_from_path(
            &self
                .inner
                .options
                .fxg_home
                .join(fxg_protocol::config::CONFIG_FILE_NAME),
        ) {
            Ok(config) => config,
            Err(err) => {
                tracing::warn!("failed to read config.toml for the settings view: {err}");
                fxg_protocol::config::GlobalConfig::default()
            }
        };
        let env = fxg_protocol::config::process_env;
        let fxg_home_source = if env(fxg_protocol::config::ENV_FXG_HOME).is_some() {
            ConfigSource::Env
        } else {
            ConfigSource::Default
        };
        let fxg_home = SourcedValue::new(
            self.inner.options.fxg_home.to_string_lossy().into_owned(),
            fxg_home_source.clone(),
            (fxg_home_source == ConfigSource::Env)
                .then(|| fxg_protocol::config::ENV_FXG_HOME.to_owned()),
        );
        let listen_addr = sourced_string(
            &env,
            self.inner.options.listen_addr.clone(),
            fxg_protocol::config::env_keys::HUB_LISTEN_ADDR,
            config.hub.listen_addr.clone(),
            "server.listen_addr",
            Some(fxg_protocol::config::HubBootstrapConfig::DEFAULT_LISTEN_ADDR),
            Some("--listen"),
        );
        let allowed_hosts = self.inner.options.allowed_hosts.clone();
        let allowed_origins = self.inner.options.allowed_origins.clone();
        let log = &self.inner.options.log;
        let log_level = sourced_string(
            &env,
            log.level.clone().unwrap_or_else(|| "info".to_owned()),
            fxg_protocol::config::env_keys::LOG_LEVEL,
            config.log.level.clone(),
            "log.level",
            Some("info"),
            Some("--log-level"),
        );
        let log_file = sourced_option(
            &env,
            log.file.clone().filter(|value| value != "-"),
            fxg_protocol::config::env_keys::LOG_FILE,
            config.log.file.clone(),
            "log.file",
            Some("--log-file"),
        );
        let log_format = sourced_string(
            &env,
            log.format.clone().unwrap_or_else(|| "text".to_owned()),
            fxg_protocol::config::env_keys::LOG_FORMAT,
            config.log.format.clone(),
            "log.format",
            Some("text"),
            Some("--log-format"),
        );
        let log_no_color = SourcedValue::new(
            log.no_color,
            if log.no_color == config.log.no_color {
                ConfigSource::ConfigFile
            } else {
                ConfigSource::Cli
            },
            Some("--no-color"),
        );
        Ok(HubSettingsSnapshot {
            bootstrap: HubBootstrapView {
                fxg_home,
                listen_addr,
                allowed_hosts: SourcedValue::new(
                    allowed_hosts,
                    if config.hub.allowed_hosts.is_empty() {
                        ConfigSource::Default
                    } else {
                        ConfigSource::ConfigFile
                    },
                    Some("server.allowed_hosts"),
                ),
                allowed_origins: SourcedValue::new(
                    allowed_origins,
                    if config.hub.allowed_origins.is_empty() {
                        ConfigSource::Default
                    } else {
                        ConfigSource::ConfigFile
                    },
                    Some("server.allowed_origins"),
                ),
                log_level,
                log_file,
                log_format,
                log_no_color,
            },
            hub: settings,
        })
    }

    /// Updates hub settings and reloads provisioners / VAPID subject.
    pub async fn update_hub_settings(
        &self,
        request: &UpdateHubSettingsRequest,
    ) -> Result<HubSettingsSnapshot, HubError> {
        let settings = self.db().update_hub_settings(request).await?;
        self.provisioners()
            .apply_settings(self.db().provisioners().await?);
        self.push().set_subject(settings.vapid_subject.as_deref());
        self.hub_settings_snapshot().await
    }

    /// Snapshot of the configured provisioners (`GET /api/v1/provisioners`).
    pub fn provisioner_summaries(&self) -> ProvisionersResponse {
        let provisioners = self
            .provisioners()
            .configs()
            .into_iter()
            .map(|(name, config)| ProvisionerSummary {
                name,
                description: config.description,
                command: config.command,
                args: config.args,
                idle_timeout_secs: config.idle_timeout_secs,
                env: config.env,
                env_secrets: config.env_secrets,
            })
            .collect();
        ProvisionersResponse { provisioners }
    }

    /// Creates or updates a provisioner and reloads the in-memory definitions.
    pub async fn save_provisioner(
        &self,
        name: &str,
        mut definition: ProvisionerDefinition,
        client: &ClientInfo,
    ) -> Result<ProvisionersResponse, ApiError> {
        validate_provisioner(name, &definition)?;
        definition.name = name.to_owned();
        self.db()
            .upsert_provisioner(&definition)
            .await
            .map_err(ApiError::from)?;
        self.provisioners()
            .apply_settings(self.db().provisioners().await.map_err(ApiError::from)?);
        self.record_audit(
            fxg_db::audit::actions::SETTINGS_UPDATE,
            client,
            None,
            serde_json::json!({ "section": "provisioner", "name": name }),
        )
        .await;
        Ok(self.provisioner_summaries())
    }

    /// Deletes a provisioner and reloads the in-memory definitions.
    pub async fn remove_provisioner(
        &self,
        name: &str,
        client: &ClientInfo,
    ) -> Result<(), ApiError> {
        let removed = self
            .db()
            .delete_provisioner(name)
            .await
            .map_err(ApiError::from)?;
        if removed == 0 {
            return Err(ApiError::not_found(format!(
                "provisioner not found: {name}"
            )));
        }
        self.provisioners()
            .apply_settings(self.db().provisioners().await.map_err(ApiError::from)?);
        self.record_audit(
            fxg_db::audit::actions::SETTINGS_UPDATE,
            client,
            None,
            serde_json::json!({ "section": "provisioner", "name": name, "deleted": true }),
        )
        .await;
        Ok(())
    }

    /// Lists hub-owned secret metadata.
    pub async fn server_secrets(&self) -> Result<SecretsResponse, HubError> {
        Ok(SecretsResponse {
            secrets: fxg_db::secrets::list_secrets(self.db(), &self.inner.options.fxg_home).await?,
        })
    }

    /// Stores a hub-owned secret.
    pub async fn upsert_server_secret(
        &self,
        id: &SecretId,
        request: &UpsertSecretRequest,
    ) -> Result<SecretsResponse, HubError> {
        Ok(SecretsResponse {
            secrets: fxg_db::secrets::upsert_secret(
                self.db(),
                &self.inner.options.fxg_home,
                id,
                request,
            )
            .await?,
        })
    }

    /// Deletes a hub-owned secret.
    pub async fn delete_server_secret(&self, id: &SecretId) -> Result<SecretsResponse, HubError> {
        Ok(SecretsResponse {
            secrets: fxg_db::secrets::delete_secret(self.db(), &self.inner.options.fxg_home, id)
                .await?,
        })
    }
}

/// Validates a provisioner name and definition before saving it.
fn validate_provisioner(name: &str, definition: &ProvisionerDefinition) -> Result<(), ApiError> {
    let valid_name = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        && !name.starts_with('.');
    if !valid_name {
        return Err(ApiError::bad_request(
            "invalid provisioner name: use 1-64 characters of [a-zA-Z0-9._-]",
        ));
    }
    if definition.command.trim().is_empty() {
        return Err(ApiError::bad_request(
            "provisioner command must not be empty",
        ));
    }
    for (key, secret) in &definition.env_secrets {
        if key.trim().is_empty() || !secret.is_valid() {
            return Err(ApiError::bad_request(format!(
                "invalid env_secrets entry: {key}"
            )));
        }
    }
    Ok(())
}

pub(super) fn secret_error(err: HubError) -> ApiError {
    match err {
        HubError::SecretInvalid(message) => ApiError::from_code(ErrorCode::SecretInvalid, message),
        HubError::Db(fxg_db::DbError::SecretInUse { id, used_by }) => ApiError::from_code(
            ErrorCode::SecretInUse,
            format!("secret {} is in use: {}", id.as_str(), used_by.join(", ")),
        ),
        HubError::Db(fxg_db::DbError::SecretInvalid(message)) => {
            ApiError::from_code(ErrorCode::SecretInvalid, message)
        }
        other => ApiError::internal(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(command: &str) -> ProvisionerDefinition {
        ProvisionerDefinition {
            command: command.to_owned(),
            ..ProvisionerDefinition::default()
        }
    }

    #[test]
    fn validates_provisioner_name_and_command() {
        assert!(validate_provisioner("local-docker", &definition("docker")).is_ok());
        assert!(validate_provisioner("", &definition("docker")).is_err());
        assert!(validate_provisioner("../escape", &definition("docker")).is_err());
        assert!(validate_provisioner("local-docker", &definition("  ")).is_err());
    }

    #[test]
    fn validates_env_secret_ids() {
        let mut valid = definition("docker");
        valid
            .env_secrets
            .insert("GH_TOKEN".to_owned(), SecretId::new("GH_TOKEN"));
        assert!(validate_provisioner("local-docker", &valid).is_ok());

        let mut invalid = definition("docker");
        invalid
            .env_secrets
            .insert("GH_TOKEN".to_owned(), SecretId::new("../escape"));
        assert!(validate_provisioner("local-docker", &invalid).is_err());
    }
}
