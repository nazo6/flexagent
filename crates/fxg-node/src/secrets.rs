//! Role-scoped secret store (`~/.flexagent/secrets/{node,server}/<id>`).
//!
//! Secret *values* never enter a database or a log line. `node.db` / `server.db`
//! keep only metadata (`secrets` table), the value file is written with mode
//! `0600` on Unix.
//!
//! Design: `docs/changelog/2026-11-09-settings-consolidation-and-project-identity.md` §3.3.

use std::path::{Path, PathBuf};

use fxg_db::Db;
use fxg_protocol::SecretId;
use fxg_protocol::config::{AUTH_TOKEN_FILE_NAME, NODE_TOKEN_FILE_NAME};
use fxg_protocol::settings::{SecretKind, SecretSummary, UpsertSecretRequest};
use fxg_protocol::util::uuid_v7;

use crate::error::NodeError;

/// Directory name owned by a local node.
pub const ROLE_NODE: &str = "node";
/// Directory name owned by the central server.
pub const ROLE_SERVER: &str = "server";

/// Value file path of a user secret.
pub fn secret_path(fxg_home: &Path, role: &str, id: &SecretId) -> PathBuf {
    fxg_home.join("secrets").join(role).join(id.as_str())
}

fn ensure_parent(path: &Path) -> Result<(), NodeError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| NodeError::io(parent, source))?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), NodeError> {
    use std::os::unix::fs::PermissionsExt;
    let permissions = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, permissions).map_err(|source| NodeError::io(path, source))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<(), NodeError> {
    // On Windows the default ACL of the user profile already restricts access;
    // a dedicated ACL would require the `windows` crate and is tracked separately.
    Ok(())
}

/// Writes a secret value (creating the role directory if needed) with `0600`.
pub fn write_secret(
    fxg_home: &Path,
    role: &str,
    id: &SecretId,
    value: &str,
) -> Result<(), NodeError> {
    let path = secret_path(fxg_home, role, id);
    ensure_parent(&path)?;
    std::fs::write(&path, value).map_err(|source| NodeError::io(&path, source))?;
    set_owner_only(&path)
}

/// Reads a secret value (`None` when the file is missing).
pub fn read_secret(
    fxg_home: &Path,
    role: &str,
    id: &SecretId,
) -> Result<Option<String>, NodeError> {
    let path = secret_path(fxg_home, role, id);
    match std::fs::read_to_string(&path) {
        Ok(value) => Ok(Some(value)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(NodeError::io(&path, source)),
    }
}

/// Deletes a secret value file (missing files are not an error).
pub fn delete_secret_file(fxg_home: &Path, role: &str, id: &SecretId) -> Result<(), NodeError> {
    let path = secret_path(fxg_home, role, id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(NodeError::io(&path, source)),
    }
}

/// Generates a fresh 32-byte hex token (values returned by rotate).
pub fn generate_secret_value() -> String {
    format!(
        "{}{}",
        uuid_v7().replace('-', ""),
        uuid_v7().replace('-', "")
    )
}

/// Fixed path of a system secret (`node_token` / `auth_token` / VAPID key).
fn system_secret_path(fxg_home: &Path, id: &SecretId) -> Option<PathBuf> {
    match id.as_str() {
        "node_token" => Some(fxg_home.join(NODE_TOKEN_FILE_NAME)),
        "auth_token" => Some(fxg_home.join(AUTH_TOKEN_FILE_NAME)),
        "vapid_private" => Some(fxg_home.join("vapid.json")),
        _ => None,
    }
}

/// Whether the value of a secret exists (system secrets have fixed paths).
pub fn is_configured(fxg_home: &Path, role: &str, id: &SecretId) -> bool {
    system_secret_path(fxg_home, id)
        .unwrap_or_else(|| secret_path(fxg_home, role, id))
        .is_file()
}

/// Registers metadata for the system secrets that already exist on disk.
pub async fn register_system_secrets(
    db: &Db,
    fxg_home: &Path,
    system: &[(&str, SecretKind)],
) -> Result<(), NodeError> {
    for (id, kind) in system {
        let id = SecretId::new(*id);
        if is_configured(fxg_home, ROLE_NODE, &id) || is_configured(fxg_home, ROLE_SERVER, &id) {
            db.upsert_secret_metadata(&id, kind.as_str(), kind).await?;
        }
    }
    Ok(())
}

/// Agent IDs (or Git hosts) that reference a secret.
async fn used_by(db: &Db, id: &SecretId) -> Result<Vec<String>, NodeError> {
    let mut used = Vec::new();
    for (agent_id, config) in db.custom_agents().await? {
        if config.env_secrets.values().any(|value| value == id) {
            used.push(agent_id);
        }
    }
    Ok(used)
}

/// Lists secret metadata for a node, marking missing value files as not configured.
pub async fn list_secrets(
    db: &Db,
    fxg_home: &Path,
    role: &str,
) -> Result<Vec<SecretSummary>, NodeError> {
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
    role: &str,
    id: &SecretId,
    request: &UpsertSecretRequest,
) -> Result<Vec<SecretSummary>, NodeError> {
    if !id.is_valid() {
        return Err(NodeError::SecretInvalid(format!(
            "invalid secret id: {}",
            id.as_str()
        )));
    }
    if request.value.is_empty() {
        return Err(NodeError::SecretInvalid(
            "secret value must not be empty".to_owned(),
        ));
    }
    if system_secret_path(fxg_home, id).is_some() {
        return Err(NodeError::SecretInvalid(format!(
            "secret {} is managed by the system",
            id.as_str()
        )));
    }
    write_secret(fxg_home, role, id, &request.value)?;
    db.upsert_secret_metadata(id, &request.name, &request.kind)
        .await?;
    list_secrets(db, fxg_home, role).await
}

/// Deletes a secret after checking that nothing references it.
pub async fn delete_secret(
    db: &Db,
    fxg_home: &Path,
    role: &str,
    id: &SecretId,
) -> Result<Vec<SecretSummary>, NodeError> {
    let referenced = used_by(db, id).await?;
    if !referenced.is_empty() {
        return Err(NodeError::SecretInUse {
            id: id.clone(),
            used_by: referenced,
        });
    }
    if system_secret_path(fxg_home, id).is_some() {
        return Err(NodeError::SecretInvalid(format!(
            "secret {} is managed by the system",
            id.as_str()
        )));
    }
    delete_secret_file(fxg_home, role, id)?;
    db.delete_secret_metadata(id).await?;
    list_secrets(db, fxg_home, role).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_db::{Db, DbRole};
    use fxg_protocol::settings::CustomAgentConfig;
    use std::collections::BTreeMap;

    async fn db() -> Db {
        Db::open_in_memory(DbRole::Node).await.expect("db")
    }

    #[tokio::test]
    async fn write_read_delete_and_configured_flag() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = db().await;
        let id = SecretId::new("agent-key");
        let secrets = upsert_secret(
            &db,
            dir.path(),
            ROLE_NODE,
            &id,
            &UpsertSecretRequest {
                name: "Agent key".to_owned(),
                kind: SecretKind::AgentEnv,
                value: "secret-value".to_owned(),
            },
        )
        .await
        .expect("upsert");
        assert!(secrets[0].configured);
        assert_eq!(
            read_secret(dir.path(), ROLE_NODE, &id).unwrap().as_deref(),
            Some("secret-value")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(secret_path(dir.path(), ROLE_NODE, &id))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "secret files must be 0600");
        }

        // Role separation: the server role path is different and empty.
        assert!(read_secret(dir.path(), ROLE_SERVER, &id).unwrap().is_none());

        let secrets = delete_secret(&db, dir.path(), ROLE_NODE, &id)
            .await
            .expect("delete");
        assert!(secrets.is_empty());
        assert!(read_secret(dir.path(), ROLE_NODE, &id).unwrap().is_none());
    }

    #[tokio::test]
    async fn referenced_secret_cannot_be_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = db().await;
        let id = SecretId::new("agent-key");
        db.upsert_custom_agent(
            "custom",
            &CustomAgentConfig {
                command: "agent".to_owned(),
                env_secrets: BTreeMap::from([("API_KEY".to_owned(), id.clone())]),
                ..Default::default()
            },
        )
        .await
        .expect("agent");
        write_secret(dir.path(), ROLE_NODE, &id, "value").expect("write");
        db.upsert_secret_metadata(&id, "Agent key", &SecretKind::AgentEnv)
            .await
            .expect("metadata");

        let err = delete_secret(&db, dir.path(), ROLE_NODE, &id)
            .await
            .expect_err("in use");
        match err {
            NodeError::SecretInUse { used_by, .. } => assert_eq!(used_by, vec!["custom"]),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn invalid_id_and_empty_value_are_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = db().await;
        let empty = UpsertSecretRequest {
            name: "x".to_owned(),
            kind: SecretKind::AgentEnv,
            value: String::new(),
        };
        assert!(matches!(
            upsert_secret(&db, dir.path(), ROLE_NODE, &SecretId::new("ok"), &empty).await,
            Err(NodeError::SecretInvalid(_))
        ));
        let valid = UpsertSecretRequest {
            name: "x".to_owned(),
            kind: SecretKind::AgentEnv,
            value: "v".to_owned(),
        };
        assert!(matches!(
            upsert_secret(
                &db,
                dir.path(),
                ROLE_NODE,
                &SecretId::new("../escape"),
                &valid
            )
            .await,
            Err(NodeError::SecretInvalid(_))
        ));
    }
}
