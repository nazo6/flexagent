//! Server-owned secret store (`~/.flexagent/secrets/server/<id>`).
//!
//! Only metadata lives in `server.db`; values stay in `0600` files.
//! Design: `docs/changelog/2026-11-09-settings-consolidation-and-project-identity.md` §3.3.

use std::path::Path;

use fxg_db::Db;
use fxg_protocol::SecretId;
use fxg_protocol::config::AUTH_TOKEN_FILE_NAME;
use fxg_protocol::settings::{SecretKind, SecretSummary, UpsertSecretRequest};
use fxg_protocol::util::uuid_v7;

use crate::error::ServerError;

/// Directory name owned by the central server.
pub const ROLE_SERVER: &str = "server";

/// Value file path of a user secret.
pub fn secret_path(fxg_home: &Path, id: &SecretId) -> std::path::PathBuf {
    fxg_home.join("secrets").join(ROLE_SERVER).join(id.as_str())
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), ServerError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<(), ServerError> {
    Ok(())
}

/// Writes a secret value with `0600`.
pub fn write_secret(fxg_home: &Path, id: &SecretId, value: &str) -> Result<(), ServerError> {
    let path = secret_path(fxg_home, id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, value)?;
    set_owner_only(&path)?;
    Ok(())
}

/// Deletes a secret value file.
pub fn delete_secret_file(fxg_home: &Path, id: &SecretId) -> Result<(), ServerError> {
    match std::fs::remove_file(secret_path(fxg_home, id)) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Generates a fresh 32-byte hex token.
pub fn generate_secret_value() -> String {
    format!(
        "{}{}",
        uuid_v7().replace('-', ""),
        uuid_v7().replace('-', "")
    )
}

/// Whether a secret is a system-managed value with a fixed path.
///
/// System secrets are registered in the metadata list but cannot be created,
/// overwritten, deleted or rotated through the Secret API.
pub fn is_system_managed(id: &SecretId) -> bool {
    matches!(id.as_str(), "auth_token" | "vapid_private")
}

/// Whether the kind is only ever created by the process itself (fixed paths).
pub fn is_system_kind(kind: &SecretKind) -> bool {
    matches!(
        kind,
        SecretKind::NodeToken | SecretKind::AuthToken | SecretKind::VapidPrivate
    )
}

fn system_secret_path(fxg_home: &Path, id: &SecretId) -> Option<std::path::PathBuf> {
    match id.as_str() {
        "auth_token" => Some(fxg_home.join(AUTH_TOKEN_FILE_NAME)),
        "vapid_private" => Some(fxg_home.join("vapid.json")),
        _ => None,
    }
}

fn is_configured(fxg_home: &Path, id: &SecretId) -> bool {
    system_secret_path(fxg_home, id)
        .unwrap_or_else(|| secret_path(fxg_home, id))
        .is_file()
}

/// Registers metadata for system secrets that already exist on disk.
pub async fn register_system_secrets(
    db: &Db,
    fxg_home: &Path,
    system: &[(&str, &str, SecretKind)],
) -> Result<(), ServerError> {
    for (id, name, kind) in system {
        let id = SecretId::new(*id);
        if is_configured(fxg_home, &id) {
            db.upsert_secret_metadata(&id, name, kind).await?;
        }
    }
    Ok(())
}

/// Git hosts that reference a secret.
async fn used_by(db: &Db, id: &SecretId) -> Result<Vec<String>, ServerError> {
    let settings = db.server_settings().await?;
    let Some(settings) = settings else {
        return Ok(Vec::new());
    };
    Ok(settings
        .git_credentials
        .into_iter()
        .filter(|(_, config)| {
            matches!(
                config,
                fxg_protocol::settings::GitCredentialConfig::Secret { secret_id, .. }
                    if secret_id == id
            )
        })
        .map(|(host, _)| host)
        .collect())
}

/// Lists server secret metadata.
pub async fn list_secrets(db: &Db, fxg_home: &Path) -> Result<Vec<SecretSummary>, ServerError> {
    let mut secrets = db.secrets().await?;
    for secret in &mut secrets {
        secret.configured = is_configured(fxg_home, &secret.id);
        secret.used_by = used_by(db, &secret.id).await?;
        if !secret.configured {
            secret.updated_at = None;
        }
    }
    Ok(secrets)
}

/// Stores a server secret value and metadata.
pub async fn upsert_secret(
    db: &Db,
    fxg_home: &Path,
    id: &SecretId,
    request: &UpsertSecretRequest,
) -> Result<Vec<SecretSummary>, ServerError> {
    if !id.is_valid() {
        return Err(ServerError::SecretInvalid(format!(
            "invalid secret id: {}",
            id.as_str()
        )));
    }
    if request.value.is_empty() {
        return Err(ServerError::SecretInvalid(
            "secret value must not be empty".to_owned(),
        ));
    }
    if is_system_kind(&request.kind) {
        return Err(ServerError::SecretInvalid(format!(
            "secret kind {} is managed by the system",
            request.kind.as_str()
        )));
    }
    // Agent environment variables are node-owned (`secrets/node/`).
    if request.kind == SecretKind::AgentEnv {
        return Err(ServerError::SecretInvalid(
            "agent environment secrets are owned by a node".to_owned(),
        ));
    }
    if system_secret_path(fxg_home, id).is_some() {
        return Err(ServerError::SecretInvalid(format!(
            "secret {} is managed by the system",
            id.as_str()
        )));
    }
    write_secret(fxg_home, id, &request.value)?;
    db.upsert_secret_metadata(id, &request.name, &request.kind)
        .await?;
    list_secrets(db, fxg_home).await
}

/// Deletes a server secret after checking references.
pub async fn delete_secret(
    db: &Db,
    fxg_home: &Path,
    id: &SecretId,
) -> Result<Vec<SecretSummary>, ServerError> {
    let referenced = used_by(db, id).await?;
    if !referenced.is_empty() {
        return Err(ServerError::Server(format!(
            "secret {} is in use: {}",
            id.as_str(),
            referenced.join(", ")
        )));
    }
    if system_secret_path(fxg_home, id).is_some() {
        return Err(ServerError::SecretInvalid(format!(
            "secret {} is managed by the system",
            id.as_str()
        )));
    }
    delete_secret_file(fxg_home, id)?;
    db.delete_secret_metadata(id).await?;
    list_secrets(db, fxg_home).await
}
