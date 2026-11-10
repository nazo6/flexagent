//! Role-scoped user secret store (`~/.flexagent/secrets/{node,hub}/<id>`).
//!
//! Secret values are stored as plaintext files with mode `0600` on Unix and
//! restricted ACLs on Windows. They never enter the SQLite database or log lines;
//! the database holds only metadata (`secrets` table: key, description, timestamps).
//!
//! Node and Hub share the same implementation parameterized by role.

use std::path::{Path, PathBuf};

use fxg_protocol::SecretId;
use fxg_protocol::settings::{SecretSummary, UpsertSecretRequest};

use crate::db::{Db, DbRole};
use crate::error::DbError;

/// Directory name owned by a local node.
pub const ROLE_NODE: &str = "node";
/// Directory name owned by the hub.
pub const ROLE_HUB: &str = "hub";

/// Maps a database role to its secrets sub-directory name.
pub fn role_name(role: DbRole) -> &'static str {
    match role {
        DbRole::Node => ROLE_NODE,
        DbRole::Hub => ROLE_HUB,
    }
}

/// Returns the value file path for a secret.
pub fn secret_path(fxg_home: &Path, role: &str, id: &SecretId) -> PathBuf {
    fxg_home.join("secrets").join(role).join(id.as_str())
}

fn ensure_parent(path: &Path) -> Result<(), DbError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(DbError::Io)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), DbError> {
    use std::os::unix::fs::PermissionsExt;
    let permissions = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, permissions).map_err(DbError::Io)
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<(), DbError> {
    Ok(())
}

/// Writes a secret value (creating parent directories if needed) with mode `0600`.
pub fn write_secret(
    fxg_home: &Path,
    role: &str,
    id: &SecretId,
    value: &str,
) -> Result<(), DbError> {
    let path = secret_path(fxg_home, role, id);
    ensure_parent(&path)?;
    std::fs::write(&path, value).map_err(DbError::Io)?;
    set_owner_only(&path)
}

/// Reads a secret value (`None` when the file is missing).
pub fn read_secret(fxg_home: &Path, role: &str, id: &SecretId) -> Result<Option<String>, DbError> {
    let path = secret_path(fxg_home, role, id);
    match std::fs::read_to_string(&path) {
        Ok(value) => Ok(Some(value)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(DbError::Io(err)),
    }
}

/// Deletes a secret value file (missing files are not treated as errors).
pub fn delete_secret_file(fxg_home: &Path, role: &str, id: &SecretId) -> Result<(), DbError> {
    let path = secret_path(fxg_home, role, id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(DbError::Io(err)),
    }
}

/// Returns whether the value file exists on disk.
pub fn is_configured(fxg_home: &Path, role: &str, id: &SecretId) -> bool {
    secret_path(fxg_home, role, id).is_file()
}

/// Identifiers of agents or Git hosts that reference a secret.
pub async fn used_by(db: &Db, id: &SecretId) -> Result<Vec<String>, DbError> {
    match db.role() {
        DbRole::Node => {
            let mut used = Vec::new();
            for (agent_id, config) in db.custom_agents().await? {
                if config.env_secrets.values().any(|value| value == id) {
                    used.push(agent_id);
                }
            }
            Ok(used)
        }
        DbRole::Hub => {
            let mut used = Vec::new();
            for (name, config) in db.provisioners().await? {
                if config.env_secrets.values().any(|value| value == id) {
                    used.push(format!("provisioner:{name}"));
                }
            }
            let settings = db.hub_settings().await?;
            if let Some(settings) = settings {
                used.extend(
                    settings
                        .git_credentials
                        .into_iter()
                        .filter(|(_, config)| {
                            matches!(
                                config,
                                fxg_protocol::settings::GitCredentialConfig::Secret { secret_id, .. }
                                    if secret_id == id
                            )
                        })
                        .map(|(host, _)| host),
                );
            }
            Ok(used)
        }
    }
}

/// Lists secret metadata for the given database and data directory.
pub async fn list_secrets(db: &Db, fxg_home: &Path) -> Result<Vec<SecretSummary>, DbError> {
    let role = role_name(db.role());
    let mut secrets = db.secrets().await?;
    for secret in &mut secrets {
        secret.configured = is_configured(fxg_home, role, &secret.id);
        secret.used_by = used_by(db, &secret.id).await?;
        if !secret.configured {
            secret.updated_at = None;
        }
    }
    Ok(secrets)
}

/// Stores a secret value and its metadata (write-only API).
pub async fn upsert_secret(
    db: &Db,
    fxg_home: &Path,
    id: &SecretId,
    request: &UpsertSecretRequest,
) -> Result<Vec<SecretSummary>, DbError> {
    if !id.is_valid() {
        return Err(DbError::SecretInvalid(format!(
            "invalid secret id: {}",
            id.as_str()
        )));
    }
    if request.value.is_empty() {
        return Err(DbError::SecretInvalid(
            "secret value must not be empty".to_owned(),
        ));
    }
    let role = role_name(db.role());
    write_secret(fxg_home, role, id, &request.value)?;
    db.upsert_secret_metadata(id, request.description.as_deref())
        .await?;
    list_secrets(db, fxg_home).await
}

/// Deletes a secret after verifying it is not referenced by any configuration.
pub async fn delete_secret(
    db: &Db,
    fxg_home: &Path,
    id: &SecretId,
) -> Result<Vec<SecretSummary>, DbError> {
    let referenced = used_by(db, id).await?;
    if !referenced.is_empty() {
        return Err(DbError::SecretInUse {
            id: id.clone(),
            used_by: referenced,
        });
    }
    let role = role_name(db.role());
    delete_secret_file(fxg_home, role, id)?;
    db.delete_secret_metadata(id).await?;
    list_secrets(db, fxg_home).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::settings::CustomAgentConfig;
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn write_read_delete_and_configured_flag() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open_in_memory(DbRole::Node).await.expect("db");
        let id = SecretId::new("agent_key");
        let secrets = upsert_secret(
            &db,
            dir.path(),
            &id,
            &UpsertSecretRequest {
                description: Some("Agent key".to_owned()),
                value: "secret-value".to_owned(),
            },
        )
        .await
        .expect("upsert");
        assert_eq!(secrets.len(), 1);
        assert!(secrets[0].configured);
        assert_eq!(secrets[0].description.as_deref(), Some("Agent key"));
        assert_eq!(
            read_secret(dir.path(), ROLE_NODE, &id).unwrap().as_deref(),
            Some("secret-value")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(secret_path(dir.path(), ROLE_NODE, &id))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        let secrets = delete_secret(&db, dir.path(), &id).await.expect("delete");
        assert!(secrets.is_empty());
        assert!(!is_configured(dir.path(), ROLE_NODE, &id));
    }

    #[tokio::test]
    async fn referenced_secret_cannot_be_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open_in_memory(DbRole::Node).await.expect("db");
        let id = SecretId::new("openai_key");
        upsert_secret(
            &db,
            dir.path(),
            &id,
            &UpsertSecretRequest {
                description: None,
                value: "sk-test".to_owned(),
            },
        )
        .await
        .expect("upsert");

        let mut env_secrets = BTreeMap::new();
        env_secrets.insert("OPENAI_API_KEY".to_owned(), id.clone());
        db.upsert_custom_agent(
            "custom-agent",
            &CustomAgentConfig {
                name: Some("Custom".to_owned()),
                command: "agent".to_owned(),
                args: Vec::new(),
                env: BTreeMap::new(),
                env_secrets,
            },
        )
        .await
        .expect("upsert agent");

        let err = delete_secret(&db, dir.path(), &id)
            .await
            .expect_err("in use");
        assert!(matches!(err, DbError::SecretInUse { .. }));
    }
}
