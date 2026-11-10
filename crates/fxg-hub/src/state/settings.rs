//! Server settings, secrets and project profiles.

use fxg_protocol::common::ErrorCode;

use crate::api::ApiError;
use crate::error::HubError;
use fxg_protocol::settings::{
    ConfigSource, HubBootstrapView, HubSettingsSnapshot, ProjectProfile, SecretsResponse,
    SourcedValue, UpdateHubSettingsRequest, UpdateProjectProfileRequest, UpsertSecretRequest,
    sourced_option, sourced_string,
};
use fxg_protocol::{ProjectId, SecretId};

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

    /// Updates hub settings and reloads provisioners / Git credentials / VAPID subject.
    pub async fn update_hub_settings(
        &self,
        request: &UpdateHubSettingsRequest,
    ) -> Result<HubSettingsSnapshot, HubError> {
        let settings = self.db().update_hub_settings(request).await?;
        self.provisioners().apply_settings(
            self.db().provisioners().await?,
            settings.git_credentials.clone(),
        );
        self.push().set_subject(settings.vapid_subject.as_deref());
        self.hub_settings_snapshot().await
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

    /// Returns the project bootstrap profile (default when absent).
    pub async fn project_profile(&self, project_id: &str) -> Result<ProjectProfile, ApiError> {
        let id = ProjectId::from(project_id);
        if let Some(profile) = self.db().project_profile(&id).await? {
            return Ok(profile);
        }
        let projects = self.db().list_projects().await?;
        if !projects.iter().any(|project| project.project_id == id) {
            return Err(ApiError::not_found(format!(
                "project not found: {project_id}"
            )));
        }
        Ok(ProjectProfile {
            project_id: id,
            bootstrap_setup: Vec::new(),
            bootstrap_env: Default::default(),
            revision: 0,
            updated_at: 0,
        })
    }

    /// Updates the project bootstrap profile.
    pub async fn update_project_profile(
        &self,
        project_id: &str,
        request: &UpdateProjectProfileRequest,
    ) -> Result<ProjectProfile, ApiError> {
        let id = ProjectId::from(project_id);
        let profile = self.db().upsert_project_profile(&id, request).await?;
        Ok(profile)
    }
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
