//! Diagnostics: component health for the local node and the hub.
//!
//! Node and server share this layer so both endpoints report the same health
//! shape (design: `docs/03-protocol-and-api.md` §3.1).

use std::path::Path;

use fxg_protocol::client_api::DiagnosticsResponse;
use fxg_protocol::common::{ComponentHealth, ComponentName, ComponentState, SystemStatus};
use fxg_protocol::util::now_ms;

use super::{ClientApiBackend, DiagnosticsCounters};

/// Free disk space below this is degraded (bytes).
const DISK_DEGRADED_BYTES: u64 = 1024 * 1024 * 1024;
/// Free disk space below this is an error (bytes).
const DISK_ERROR_BYTES: u64 = 100 * 1024 * 1024;

/// Builds the component health snapshot for an endpoint.
pub(crate) async fn build_system_status<B: ClientApiBackend>(backend: &B) -> SystemStatus {
    let now = now_ms();
    let mut components = Vec::new();

    match backend.db().health_check().await {
        Ok(()) => {
            components.push(ComponentHealth::ok(ComponentName::Db, now));
            components.push(ComponentHealth::ok(ComponentName::Migrations, now));
        }
        Err(err) => {
            let message = format!("database unavailable: {err}");
            components.push(ComponentHealth::failed(
                ComponentName::Db,
                ComponentState::Error,
                &message,
                now,
            ));
            components.push(ComponentHealth::failed(
                ComponentName::Migrations,
                ComponentState::Error,
                message,
                now,
            ));
        }
    }

    components.push(ComponentHealth::ok(ComponentName::Http, now));
    components.extend(backend.role_health(now).await);
    components.push(disk_health(backend.data_dir().as_deref(), now));

    let counters = backend
        .diagnostics_counters()
        .map(DiagnosticsCounters::snapshot)
        .unwrap_or_default();

    SystemStatus {
        checked_at: now,
        components,
        counters,
    }
}

/// Free-space check of the data directory.
fn disk_health(home: Option<&Path>, now: i64) -> ComponentHealth {
    let Some(home) = home else {
        return ComponentHealth {
            component: ComponentName::Disk,
            state: ComponentState::NotMeasured,
            message: Some("data directory is unknown".to_owned()),
            last_ok_at: None,
            last_error_at: None,
        };
    };
    match fs4::available_space(home) {
        Ok(available) if available < DISK_ERROR_BYTES => ComponentHealth::failed(
            ComponentName::Disk,
            ComponentState::Error,
            format!("only {available} bytes are free in the data directory"),
            now,
        ),
        Ok(available) if available < DISK_DEGRADED_BYTES => ComponentHealth::failed(
            ComponentName::Disk,
            ComponentState::Degraded,
            format!("only {available} bytes are free in the data directory"),
            now,
        ),
        Ok(_) => ComponentHealth::ok(ComponentName::Disk, now),
        Err(err) => ComponentHealth::failed(
            ComponentName::Disk,
            ComponentState::NotMeasured,
            format!("failed to read free disk space: {err}"),
            now,
        ),
    }
}

/// Builds the shared diagnostics response (used by `GET /api/v1/diagnostics`).
pub(crate) async fn build_diagnostics<B: ClientApiBackend>(
    backend: &B,
) -> Result<DiagnosticsResponse, super::ApiError> {
    let health = build_system_status(backend).await;
    let agents = backend.agent_diagnostics();
    let node_log = backend
        .log_path()
        .map(|path| path.to_string_lossy().into_owned());
    Ok(DiagnosticsResponse {
        node_id: backend
            .local_node_id()
            .map(|node_id| node_id.as_str().to_owned())
            .unwrap_or_else(|| "hub".to_owned()),
        generated_at: now_ms(),
        agents,
        node_log,
        health,
    })
}

/// Resolves the newest file for a logically named log (`node.log`).
///
/// Daily rotation writes `node.log.YYYY-MM-DD`, so callers that only know the
/// configured path use this helper instead of assuming the exact file exists.
pub fn resolve_log_file(path: &Path) -> std::path::PathBuf {
    if path.is_file() {
        return path.to_path_buf();
    }
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return path.to_path_buf();
    };
    let prefix = format!("{name}.");
    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(parent).into_iter().flatten().flatten() {
        let file_name = entry.file_name();
        if !file_name.to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        if newest
            .as_ref()
            .is_none_or(|(current, _)| modified > *current)
        {
            newest = Some((modified, entry.path()));
        }
    }
    newest
        .map(|(_, path)| path)
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_log_file_finds_a_rotated_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("node.log");
        let rotated = dir.path().join("node.log.2026-11-16");
        std::fs::write(&rotated, "rotated").expect("write rotated");
        assert_eq!(resolve_log_file(&base), rotated);
    }
}
