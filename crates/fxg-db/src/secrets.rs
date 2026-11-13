//! Role-scoped user secret store (`<data>/<role>/secrets/<id>`).
//!
//! Secret values are stored as plaintext files with mode `0600` on Unix and
//! restricted ACLs on Windows. They never enter the SQLite database or log lines;
//! the database holds only metadata (`secrets` table: key, description, timestamps).
//!
//! Node and Hub share the same implementation parameterized by [`Paths`], which
//! scopes every value to the owning role.

use std::path::{Path, PathBuf};

use fxg_protocol::SecretId;
use fxg_protocol::paths::{Paths, Role};
use fxg_protocol::settings::{SecretSummary, UpsertSecretRequest};

use crate::db::Db;
use crate::error::DbError;

/// Secret ID of the node ⇔ hub pairing token (a system secret).
pub const SYSTEM_NODE_TOKEN: &str = "system.node_token";

/// Secret ID of the Web access token (a system secret).
pub const SYSTEM_AUTH_TOKEN: &str = "system.auth_token";

/// Returns the fixed path of a system secret, if `id` is one for this role.
pub fn system_secret_path(paths: &Paths, id: &SecretId) -> Option<PathBuf> {
    match id.as_str() {
        SYSTEM_NODE_TOKEN if paths.role() == Role::Node => Some(paths.node_token_path()),
        SYSTEM_AUTH_TOKEN => Some(paths.auth_token_path()),
        _ => None,
    }
}

/// Returns the value file path for a secret.
pub fn secret_path(paths: &Paths, id: &SecretId) -> PathBuf {
    system_secret_path(paths, id).unwrap_or_else(|| paths.secrets_dir().join(id.as_str()))
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
pub fn write_secret(paths: &Paths, id: &SecretId, value: &str) -> Result<(), DbError> {
    let path = secret_path(paths, id);
    ensure_parent(&path)?;
    std::fs::write(&path, value).map_err(DbError::Io)?;
    set_owner_only(&path)
}

/// Reads a secret value (`None` when the file is missing).
pub fn read_secret(paths: &Paths, id: &SecretId) -> Result<Option<String>, DbError> {
    let path = secret_path(paths, id);
    match std::fs::read_to_string(&path) {
        Ok(value) => Ok(Some(value)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(DbError::Io(err)),
    }
}

/// Deletes a secret value file (missing files are not treated as errors).
pub fn delete_secret_file(paths: &Paths, id: &SecretId) -> Result<(), DbError> {
    let path = secret_path(paths, id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(DbError::Io(err)),
    }
}

/// Returns whether the value file exists on disk.
pub fn is_configured(paths: &Paths, id: &SecretId) -> bool {
    secret_path(paths, id).is_file()
}

/// Registers metadata for a system secret so it appears in the secret list.
pub async fn ensure_system_secret(
    db: &Db,
    id: &SecretId,
    description: &str,
) -> Result<(), DbError> {
    let now = fxg_protocol::util::now_ms();
    sqlx::query!(
        r#"
        INSERT INTO secrets (id, description, created_at, updated_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(id) DO NOTHING
        "#,
        id.as_str(),
        description,
        now,
        now,
    )
    .execute(db.pool())
    .await?;
    Ok(())
}

/// Identifiers of agents or Git hosts that reference a secret.
pub async fn used_by(db: &Db, id: &SecretId) -> Result<Vec<String>, DbError> {
    match db.role() {
        Role::Node => {
            let mut used = Vec::new();
            for (agent_id, config) in db.custom_agents().await? {
                if config.env_secrets.values().any(|value| value == id) {
                    used.push(agent_id);
                }
            }
            Ok(used)
        }
        Role::Hub => {
            let mut used = Vec::new();
            for (name, config) in db.provisioners().await? {
                if config.env_secrets.values().any(|value| value == id) {
                    used.push(format!("provisioner:{name}"));
                }
            }
            Ok(used)
        }
    }
}

/// Lists secret metadata for the given database and role paths.
pub async fn list_secrets(db: &Db, paths: &Paths) -> Result<Vec<SecretSummary>, DbError> {
    debug_assert_eq!(db.role(), paths.role());
    let mut secrets = db.secrets().await?;
    for secret in &mut secrets {
        secret.configured = is_configured(paths, &secret.id);
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
    paths: &Paths,
    id: &SecretId,
    request: &UpsertSecretRequest,
) -> Result<Vec<SecretSummary>, DbError> {
    if !id.is_valid() {
        return Err(DbError::SecretInvalid(format!(
            "invalid secret id: {}",
            id.as_str()
        )));
    }
    if system_secret_path(paths, id).is_some() {
        return Err(DbError::SecretInvalid(format!(
            "{} is a system secret and cannot be written through the secret API",
            id.as_str()
        )));
    }
    if request.value.is_empty() {
        return Err(DbError::SecretInvalid(
            "secret value must not be empty".to_owned(),
        ));
    }
    write_secret(paths, id, &request.value)?;
    db.upsert_secret_metadata(id, request.description.as_deref())
        .await?;
    list_secrets(db, paths).await
}

/// Deletes a secret after verifying it is not referenced by any configuration.
pub async fn delete_secret(
    db: &Db,
    paths: &Paths,
    id: &SecretId,
) -> Result<Vec<SecretSummary>, DbError> {
    if system_secret_path(paths, id).is_some() {
        return Err(DbError::SecretInvalid(format!(
            "{} is a system secret and cannot be deleted through the secret API",
            id.as_str()
        )));
    }
    let referenced = used_by(db, id).await?;
    if !referenced.is_empty() {
        return Err(DbError::SecretInUse {
            id: id.clone(),
            used_by: referenced,
        });
    }
    delete_secret_file(paths, id)?;
    db.delete_secret_metadata(id).await?;
    list_secrets(db, paths).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::settings::CustomAgentConfig;
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn write_read_delete_and_configured_flag() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::portable(dir.path(), Role::Node);
        let db = Db::open_in_memory(Role::Node).await.expect("db");
        let id = SecretId::new("agent_key");
        let secrets = upsert_secret(
            &db,
            &paths,
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
            read_secret(&paths, &id).unwrap().as_deref(),
            Some("secret-value")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(secret_path(&paths, &id))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        let secrets = delete_secret(&db, &paths, &id).await.expect("delete");
        assert!(secrets.is_empty());
        assert!(!is_configured(&paths, &id));
    }

    #[tokio::test]
    async fn referenced_secret_cannot_be_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::portable(dir.path(), Role::Node);
        let db = Db::open_in_memory(Role::Node).await.expect("db");
        let id = SecretId::new("openai_key");
        upsert_secret(
            &db,
            &paths,
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
                extension: None,
                ..Default::default()
            },
        )
        .await
        .expect("upsert agent");

        let err = delete_secret(&db, &paths, &id).await.expect_err("in use");
        assert!(matches!(err, DbError::SecretInUse { .. }));
    }
}
