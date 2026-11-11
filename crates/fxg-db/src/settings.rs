//! Typed settings and secret metadata persistence.
//!
//! `node.db` owns `node_settings` / `agent_settings` / `agent_aliases` /
//! `custom_agents` / `project_path_links`.
//! `hub.db` owns `hub_settings` / `provisioners`.
//!
//! Every mutable single-row/aggregate table carries an explicit `revision`
//! column; updates take the revision the client observed and fail with
//! [`DbError::RevisionMismatch`] when it moved in the meantime.

use std::collections::BTreeMap;

use fxg_protocol::SecretId;
use fxg_protocol::settings::{
    AgentSettings, CustomAgentConfig, CustomAgentOrigin, HubSettings, NodeSettings,
    ProvisionerDefinition, SecretSummary, UpdateAgentSettingsRequest, UpdateHubSettingsRequest,
    UpdateNodeSettingsRequest,
};
use fxg_protocol::util::now_ms;
use sqlx::SqlitePool;

use crate::error::DbError;

fn revision_u64(revision: i64) -> Result<u64, DbError> {
    u64::try_from(revision)
        .map_err(|err| DbError::InvalidPayload(format!("negative revision: {err}")))
}

fn positive_u64(value: i64) -> Option<u64> {
    match u64::try_from(value) {
        Ok(value) if value > 0 => Some(value),
        _ => None,
    }
}

// ----------------------------------------------------------------------
// Node settings
// ----------------------------------------------------------------------

/// Ensures the single `node_settings` row exists and returns it.
pub async fn ensure_node_settings(
    pool: &SqlitePool,
    default_name: &str,
) -> Result<NodeSettings, DbError> {
    let now = now_ms();
    sqlx::query!(
        r#"
        INSERT INTO node_settings
            (id, name, allow_remote_pty, project_scan_dirs_json, tray, revision, updated_at)
        VALUES (1, ?, 0, '[]', NULL, 1, ?)
        ON CONFLICT(id) DO NOTHING
        "#,
        default_name,
        now,
    )
    .execute(pool)
    .await?;
    get_node_settings(pool)
        .await?
        .ok_or_else(|| DbError::InvalidPayload("node_settings row is missing".to_owned()))
}

/// Returns the `node_settings` row when it exists.
pub async fn get_node_settings(pool: &SqlitePool) -> Result<Option<NodeSettings>, DbError> {
    let row = sqlx::query!(
        r#"
        SELECT name, allow_remote_pty, project_scan_dirs_json, tray, revision, updated_at
          FROM node_settings
         WHERE id = 1
        "#,
    )
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let project_scan_dirs: Vec<String> = serde_json::from_str(&row.project_scan_dirs_json)?;
    Ok(Some(NodeSettings {
        name: row.name,
        allow_remote_pty: row.allow_remote_pty != 0,
        project_scan_dirs,
        tray: row.tray.map(|value| value != 0),
        revision: revision_u64(row.revision)?,
        updated_at: row.updated_at,
    }))
}

/// Updates the node settings under an optimistic lock.
pub async fn update_node_settings(
    pool: &SqlitePool,
    request: &UpdateNodeSettingsRequest,
) -> Result<NodeSettings, DbError> {
    let scan_dirs_json = serde_json::to_string(&request.project_scan_dirs)?;
    let expected = i64::try_from(request.expected_revision).unwrap_or(i64::MAX);
    let tray = request.tray.map(|value| if value { 1 } else { 0 });
    let result = sqlx::query!(
        r#"
        UPDATE node_settings
           SET name = ?,
               allow_remote_pty = ?,
               project_scan_dirs_json = ?,
               tray = ?,
               revision = revision + 1,
               updated_at = ?
         WHERE id = 1 AND revision = ?
        "#,
        request.name,
        request.allow_remote_pty,
        scan_dirs_json,
        tray,
        now_ms(),
        expected,
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::RevisionMismatch);
    }
    get_node_settings(pool)
        .await?
        .ok_or_else(|| DbError::InvalidPayload("node_settings row is missing".to_owned()))
}

// ----------------------------------------------------------------------
// Agent settings
// ----------------------------------------------------------------------

/// Ensures the single `agent_settings` row exists and returns it with aliases.
pub async fn ensure_agent_settings(pool: &SqlitePool) -> Result<AgentSettings, DbError> {
    sqlx::query!(
        r#"
        INSERT INTO agent_settings
            (id, registry_url, registry_cache_ttl_secs, revision, updated_at)
        VALUES (1, NULL, NULL, 1, ?)
        ON CONFLICT(id) DO NOTHING
        "#,
        now_ms(),
    )
    .execute(pool)
    .await?;
    get_agent_settings(pool)
        .await?
        .ok_or_else(|| DbError::InvalidPayload("agent_settings row is missing".to_owned()))
}

/// Returns the `agent_settings` row (with aliases) when it exists.
pub async fn get_agent_settings(pool: &SqlitePool) -> Result<Option<AgentSettings>, DbError> {
    let row = sqlx::query!(
        r#"
        SELECT registry_url, registry_cache_ttl_secs, revision, updated_at
          FROM agent_settings
         WHERE id = 1
        "#,
    )
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let aliases = list_agent_aliases(pool).await?;
    Ok(Some(AgentSettings {
        registry_url: row.registry_url,
        registry_cache_ttl_secs: row.registry_cache_ttl_secs.and_then(positive_u64),
        aliases,
        revision: revision_u64(row.revision)?,
        updated_at: row.updated_at,
    }))
}

/// Lists the configured agent aliases.
pub async fn list_agent_aliases(pool: &SqlitePool) -> Result<BTreeMap<String, String>, DbError> {
    let rows =
        sqlx::query!(r#"SELECT alias AS "alias!", agent_id FROM agent_aliases ORDER BY alias"#,)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.alias, row.agent_id))
        .collect())
}

/// Updates agent settings and aliases under an optimistic lock.
pub async fn update_agent_settings(
    pool: &SqlitePool,
    request: &UpdateAgentSettingsRequest,
) -> Result<AgentSettings, DbError> {
    let now = now_ms();
    let expected = i64::try_from(request.expected_revision).unwrap_or(i64::MAX);
    let ttl = request
        .registry_cache_ttl_secs
        .map(|value| i64::try_from(value).unwrap_or(i64::MAX));
    let mut tx = pool.begin().await?;
    let result = sqlx::query!(
        r#"
        UPDATE agent_settings
           SET registry_url = ?,
               registry_cache_ttl_secs = ?,
               revision = revision + 1,
               updated_at = ?
         WHERE id = 1 AND revision = ?
        "#,
        request.registry_url,
        ttl,
        now,
        expected,
    )
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::RevisionMismatch);
    }
    sqlx::query!(r#"DELETE FROM agent_aliases"#)
        .execute(&mut *tx)
        .await?;
    for (alias, agent_id) in &request.aliases {
        sqlx::query!(
            r#"INSERT INTO agent_aliases (alias, agent_id, updated_at) VALUES (?, ?, ?)"#,
            alias,
            agent_id,
            now,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    get_agent_settings(pool)
        .await?
        .ok_or_else(|| DbError::InvalidPayload("agent_settings row is missing".to_owned()))
}

// ----------------------------------------------------------------------
// Custom agents
// ----------------------------------------------------------------------

/// Lists every custom agent definition.
pub async fn list_custom_agents(
    pool: &SqlitePool,
) -> Result<BTreeMap<String, CustomAgentConfig>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT agent_id AS "agent_id!", name, command, args_json, env_json, env_secrets_json,
               extension, description, icon, origin_json
          FROM custom_agents
         ORDER BY agent_id
        "#,
    )
    .fetch_all(pool)
    .await?;
    let mut agents = BTreeMap::new();
    for row in rows {
        let args: Vec<String> = serde_json::from_str(&row.args_json)?;
        let env: BTreeMap<String, String> = serde_json::from_str(&row.env_json)?;
        let env_secrets: BTreeMap<String, SecretId> = serde_json::from_str(&row.env_secrets_json)?;
        let origin: Option<CustomAgentOrigin> = match row.origin_json {
            Some(json) => Some(serde_json::from_str(&json)?),
            None => None,
        };
        agents.insert(
            row.agent_id,
            CustomAgentConfig {
                name: row.name,
                command: row.command,
                args,
                env,
                env_secrets,
                extension: row.extension,
                description: row.description,
                icon: row.icon,
                origin,
            },
        );
    }
    Ok(agents)
}

/// Inserts or replaces a custom agent definition.
pub async fn upsert_custom_agent(
    pool: &SqlitePool,
    agent_id: &str,
    config: &CustomAgentConfig,
) -> Result<(), DbError> {
    if let Some(duplicate) = config
        .env_secrets
        .keys()
        .find(|name| config.env.contains_key(*name))
    {
        return Err(DbError::InvalidPayload(format!(
            "environment variable {duplicate} is defined both as plain and secret"
        )));
    }
    let args_json = serde_json::to_string(&config.args)?;
    let env_json = serde_json::to_string(&config.env)?;
    let env_secrets_json = serde_json::to_string(&config.env_secrets)?;
    let origin_json = match &config.origin {
        Some(origin) => Some(serde_json::to_string(origin)?),
        None => None,
    };
    sqlx::query!(
        r#"
        INSERT INTO custom_agents
            (agent_id, name, command, args_json, env_json, env_secrets_json, extension,
             description, icon, origin_json, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(agent_id) DO UPDATE SET
            name = excluded.name,
            command = excluded.command,
            args_json = excluded.args_json,
            env_json = excluded.env_json,
            env_secrets_json = excluded.env_secrets_json,
            extension = excluded.extension,
            description = excluded.description,
            icon = excluded.icon,
            origin_json = excluded.origin_json,
            updated_at = excluded.updated_at
        "#,
        agent_id,
        config.name,
        config.command,
        args_json,
        env_json,
        env_secrets_json,
        config.extension,
        config.description,
        config.icon,
        origin_json,
        now_ms(),
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Deletes a custom agent definition (returns the number of deleted rows).
pub async fn delete_custom_agent(pool: &SqlitePool, agent_id: &str) -> Result<u64, DbError> {
    let result = sqlx::query!(r#"DELETE FROM custom_agents WHERE agent_id = ?"#, agent_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

// ----------------------------------------------------------------------
// Project path links
// ----------------------------------------------------------------------

/// Returns the explicit project key linked to a local path (exact match).
pub async fn find_project_key(
    pool: &SqlitePool,
    local_path: &str,
) -> Result<Option<String>, DbError> {
    let row = sqlx::query!(
        r#"SELECT project_key FROM project_path_links WHERE local_path = ?"#,
        local_path,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row.project_key))
}

/// Lists every explicit path link (`local_path`, `project_key`).
pub async fn list_project_path_links(pool: &SqlitePool) -> Result<Vec<(String, String)>, DbError> {
    let rows = sqlx::query!(
        r#"SELECT local_path AS "local_path!", project_key FROM project_path_links ORDER BY local_path"#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.local_path, row.project_key))
        .collect())
}

/// Inserts or replaces the explicit link of a local path.
pub async fn upsert_project_path_link(
    pool: &SqlitePool,
    local_path: &str,
    project_key: &str,
) -> Result<(), DbError> {
    let now = now_ms();
    sqlx::query!(
        r#"
        INSERT INTO project_path_links (local_path, project_key, created_at, updated_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(local_path) DO UPDATE SET
            project_key = excluded.project_key,
            updated_at = excluded.updated_at
        "#,
        local_path,
        project_key,
        now,
        now,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Deletes the explicit link of a local path (returns the number of deleted rows).
pub async fn delete_project_path_link(pool: &SqlitePool, local_path: &str) -> Result<u64, DbError> {
    let result = sqlx::query!(
        r#"DELETE FROM project_path_links WHERE local_path = ?"#,
        local_path,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

// ----------------------------------------------------------------------
// Server settings
// ----------------------------------------------------------------------

/// Ensures the single `hub_settings` row exists and returns it.
pub async fn ensure_hub_settings(pool: &SqlitePool) -> Result<HubSettings, DbError> {
    sqlx::query!(
        r#"
        INSERT INTO hub_settings (id, vapid_subject, revision, updated_at)
        VALUES (1, NULL, 1, ?)
        ON CONFLICT(id) DO NOTHING
        "#,
        now_ms(),
    )
    .execute(pool)
    .await?;
    get_hub_settings(pool)
        .await?
        .ok_or_else(|| DbError::InvalidPayload("hub_settings row is missing".to_owned()))
}

/// Returns the `hub_settings` row when it exists.
pub async fn get_hub_settings(pool: &SqlitePool) -> Result<Option<HubSettings>, DbError> {
    let row = sqlx::query!(
        r#"SELECT vapid_subject, revision, updated_at FROM hub_settings WHERE id = 1"#,
    )
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(HubSettings {
        vapid_subject: row.vapid_subject,
        revision: revision_u64(row.revision)?,
        updated_at: row.updated_at,
    }))
}

/// Updates hub settings under an optimistic lock.
pub async fn update_hub_settings(
    pool: &SqlitePool,
    request: &UpdateHubSettingsRequest,
) -> Result<HubSettings, DbError> {
    let now = now_ms();
    let expected = i64::try_from(request.expected_revision).unwrap_or(i64::MAX);
    let result = sqlx::query!(
        r#"
        UPDATE hub_settings
           SET vapid_subject = ?, revision = revision + 1, updated_at = ?
         WHERE id = 1 AND revision = ?
        "#,
        request.vapid_subject,
        now,
        expected,
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::RevisionMismatch);
    }
    get_hub_settings(pool)
        .await?
        .ok_or_else(|| DbError::InvalidPayload("hub_settings row is missing".to_owned()))
}

// ----------------------------------------------------------------------
// Provisioners
// ----------------------------------------------------------------------

/// Lists every provisioner definition.
pub async fn list_provisioners(
    pool: &SqlitePool,
) -> Result<BTreeMap<String, ProvisionerDefinition>, DbError> {
    let rows = sqlx::query!(
        r#"
        SELECT name AS "name!", description, command, args_json, idle_timeout_secs, env_json,
               env_secrets_json
          FROM provisioners
         ORDER BY name
        "#,
    )
    .fetch_all(pool)
    .await?;
    let mut provisioners = BTreeMap::new();
    for row in rows {
        provisioners.insert(
            row.name.clone(),
            ProvisionerDefinition {
                name: row.name,
                description: row.description,
                command: row.command,
                args: serde_json::from_str(&row.args_json)?,
                idle_timeout_secs: row.idle_timeout_secs.and_then(positive_u64),
                env: serde_json::from_str(&row.env_json)?,
                env_secrets: serde_json::from_str(&row.env_secrets_json)?,
            },
        );
    }
    Ok(provisioners)
}

/// Inserts or replaces a provisioner definition.
pub async fn upsert_provisioner(
    pool: &SqlitePool,
    definition: &ProvisionerDefinition,
) -> Result<(), DbError> {
    let args_json = serde_json::to_string(&definition.args)?;
    let env_json = serde_json::to_string(&definition.env)?;
    let env_secrets_json = serde_json::to_string(&definition.env_secrets)?;
    let idle_timeout_secs = definition
        .idle_timeout_secs
        .map(|value| i64::try_from(value).unwrap_or(i64::MAX));
    sqlx::query!(
        r#"
        INSERT INTO provisioners
            (name, description, command, args_json, idle_timeout_secs, env_json,
             env_secrets_json, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(name) DO UPDATE SET
            description = excluded.description,
            command = excluded.command,
            args_json = excluded.args_json,
            idle_timeout_secs = excluded.idle_timeout_secs,
            env_json = excluded.env_json,
            env_secrets_json = excluded.env_secrets_json,
            updated_at = excluded.updated_at
        "#,
        definition.name,
        definition.description,
        definition.command,
        args_json,
        idle_timeout_secs,
        env_json,
        env_secrets_json,
        now_ms(),
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Deletes a provisioner definition (returns the number of deleted rows).
pub async fn delete_provisioner(pool: &SqlitePool, name: &str) -> Result<u64, DbError> {
    let result = sqlx::query!(r#"DELETE FROM provisioners WHERE name = ?"#, name)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

// ----------------------------------------------------------------------
// Secret metadata
// ----------------------------------------------------------------------

/// Lists secret metadata (values are not stored in the DB).
pub async fn list_secrets(pool: &SqlitePool) -> Result<Vec<SecretSummary>, DbError> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id!", description, created_at, updated_at FROM secrets ORDER BY id"#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| SecretSummary {
            id: SecretId::new(row.id),
            description: row.description,
            configured: true,
            updated_at: Some(row.updated_at),
            used_by: Vec::new(),
        })
        .collect())
}

/// Returns the metadata of a single secret.
pub async fn get_secret(
    pool: &SqlitePool,
    id: &SecretId,
) -> Result<Option<SecretSummary>, DbError> {
    let row = sqlx::query!(
        r#"SELECT id AS "id!", description, created_at, updated_at FROM secrets WHERE id = ?"#,
        id.as_str(),
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| SecretSummary {
        id: SecretId::new(row.id),
        description: row.description,
        configured: true,
        updated_at: Some(row.updated_at),
        used_by: Vec::new(),
    }))
}

/// Inserts or updates secret metadata.
pub async fn upsert_secret_metadata(
    pool: &SqlitePool,
    id: &SecretId,
    description: Option<&str>,
) -> Result<(), DbError> {
    let now = now_ms();
    sqlx::query!(
        r#"
        INSERT INTO secrets (id, description, created_at, updated_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            description = excluded.description,
            updated_at = excluded.updated_at
        "#,
        id.as_str(),
        description,
        now,
        now,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Deletes secret metadata (returns the number of deleted rows).
pub async fn delete_secret_metadata(pool: &SqlitePool, id: &SecretId) -> Result<u64, DbError> {
    let result = sqlx::query!(r#"DELETE FROM secrets WHERE id = ?"#, id.as_str())
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}
