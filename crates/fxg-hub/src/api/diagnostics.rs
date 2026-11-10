//! Diagnostics: component health, incident recording and support bundles.
//!
//! Design: `docs/changelog/2026-11-08-error-handling-and-recovery-plan.md`
//! Phase 3. Node and server share this layer so both endpoints report the same
//! health shape and produce the same redacted support bundle.

#![allow(
    clippy::disallowed_types,
    reason = "recursively redact intentionally opaque RawJson incident details at the diagnostics boundary"
)]

use std::path::Path;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use fxg_db::incidents::{INCIDENT_MAX_ENTRIES, INCIDENT_RETENTION_DAYS};
use fxg_protocol::client_api::{
    ConnectionRole, DatabaseStats, DiagnosticsResponse, SupportBundle, SupportBundleConfig,
};
use fxg_protocol::common::{
    ComponentHealth, ComponentName, ComponentState, ErrorCode, ErrorInfo, IncidentEntry,
    IncidentLevel, IncidentSource, RawJson, SystemStatus, WarningInfo,
};
use fxg_protocol::util::now_ms;

use super::{ClientApiBackend, ClientApiState, DiagnosticsCounters, header_str};

/// Window in which unresolved incidents make a component degraded.
const HEALTH_INCIDENT_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
/// Free disk space below this is degraded (bytes).
const DISK_DEGRADED_BYTES: u64 = 1024 * 1024 * 1024;
/// Free disk space below this is an error (bytes).
const DISK_ERROR_BYTES: u64 = 100 * 1024 * 1024;
/// Number of log lines included in a support bundle.
const SUPPORT_BUNDLE_LOG_LINES: usize = 200;
/// Number of sessions included in a support bundle.
const SUPPORT_BUNDLE_SESSION_LIMIT: u32 = 50;

/// Persists failures returned by the REST boundary as incidents.
///
/// The response carries `x-fxg-error-code` (set by `ApiError::into_response`),
/// so the middleware never buffers or inspects the response body. Client errors
/// that are part of normal operation (`400` / `401` / `403` / `404`) are ignored;
/// server errors and recoverable conflicts are recorded.
pub(crate) async fn record_api_errors<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let request_id = super::request_context::current_request_id();
    let response = next.run(request).await;

    if matches!(
        path.as_str(),
        "/healthz" | "/readyz" | "/api/v1/meta" | "/api/v1/diagnostics"
    ) || !should_record_status(response.status())
    {
        return response;
    }
    let Some(code) = error_code_from_headers(response.headers()) else {
        return response;
    };
    let level = if response.status().is_server_error() {
        IncidentLevel::Error
    } else {
        IncidentLevel::Warning
    };
    let info = ErrorInfo {
        code,
        message: format!("{method} {path} failed"),
        params: std::collections::BTreeMap::new(),
        details: Some(RawJson::new(serde_json::json!({
            "method": method.as_str(),
            "path": path,
            "status": response.status().as_u16(),
        }))),
        retryable: code.is_retryable(),
        recovery: Vec::new(),
        request_id: request_id.clone(),
        data_safety: None,
        docs: None,
    };
    record_info(
        &state.backend,
        &info,
        IncidentSource::Api,
        level,
        session_id_from_path(&path),
        request_id.as_deref(),
    )
    .await;
    response
}

/// Builds the component health snapshot for an endpoint.
pub(crate) async fn build_system_status<B: ClientApiBackend>(backend: &B) -> SystemStatus {
    let now = now_ms();
    let mut components = Vec::new();

    match backend.db().database_counts().await {
        Ok(_) => {
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
    components.push(disk_health(backend.fxg_home(), now));

    // Recent unresolved incidents make the matching component degraded / failing.
    let since = now - HEALTH_INCIDENT_WINDOW_MS;
    match backend.db().unresolved_incidents_since(since).await {
        Ok(incidents) => apply_incidents(&mut components, &incidents, now),
        Err(err) => {
            components.push(ComponentHealth::failed(
                ComponentName::Db,
                ComponentState::Degraded,
                format!("failed to load incidents: {err}"),
                now,
            ));
        }
    }

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

/// Adds / replaces components based on recent incidents.
fn apply_incidents(components: &mut Vec<ComponentHealth>, incidents: &[IncidentEntry], now: i64) {
    for incident in incidents {
        let Some(component) = component_for_source(&incident.source) else {
            continue;
        };
        let state = match incident.level {
            IncidentLevel::Error => ComponentState::Error,
            IncidentLevel::Warning | IncidentLevel::Info | IncidentLevel::Unknown(_) => {
                ComponentState::Degraded
            }
        };
        let health = ComponentHealth::failed(
            component.clone(),
            state,
            format!(
                "{}: {} ({} occurrence(s))",
                incident.code, incident.summary, incident.occurrence_count
            ),
            now,
        );
        match components
            .iter_mut()
            .find(|existing| existing.component == component)
        {
            Some(existing) => *existing = health,
            None => components.push(health),
        }
    }
}

/// Maps an incident source to the component it degrades.
fn component_for_source(source: &IncidentSource) -> Option<ComponentName> {
    match source {
        IncidentSource::Api => Some(ComponentName::Http),
        IncidentSource::Agent => Some(ComponentName::AgentSpawn),
        IncidentSource::Sync => Some(ComponentName::Sync),
        IncidentSource::Git => Some(ComponentName::Git),
        IncidentSource::Db => Some(ComponentName::Db),
        IncidentSource::Pty => Some(ComponentName::Pty),
        IncidentSource::Registry => Some(ComponentName::Registry),
        IncidentSource::Provisioner => Some(ComponentName::Provisioner),
        IncidentSource::Ipc => Some(ComponentName::Ipc),
        IncidentSource::Push => Some(ComponentName::Push),
        IncidentSource::Service => Some(ComponentName::Service),
        IncidentSource::Unknown(_) => None,
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

/// Records an `ErrorInfo` at an error boundary.
pub(crate) async fn record_info<B: ClientApiBackend>(
    backend: &B,
    info: &ErrorInfo,
    source: IncidentSource,
    level: IncidentLevel,
    session_id: Option<&str>,
    request_id: Option<&str>,
) {
    let node_id = backend.local_node_id();
    let mut record = fxg_db::IncidentRecord::new(level, info.code, source, &info.message)
        .with_session(session_id)
        .with_node_id(node_id.as_ref().map(fxg_protocol::NodeId::as_str))
        .with_request_id(request_id.or(info.request_id.as_deref()))
        .with_details(RawJson::new(serde_json::json!({
            "retryable": info.retryable,
            "params": info.params,
            "details": info.details,
        })));
    record.details = redact_json(&record.details, backend.fxg_home());
    backend.db().record_incident_best_effort(&record).await;
}

/// Records a warning returned with a partially successful command.
pub(crate) async fn record_warning<B: ClientApiBackend>(
    backend: &B,
    warning: &WarningInfo,
    session_id: Option<&str>,
) {
    let info = ErrorInfo {
        code: warning.code,
        message: warning.message.clone(),
        params: std::collections::BTreeMap::new(),
        details: warning.details.clone(),
        retryable: false,
        recovery: warning.recovery.clone(),
        request_id: None,
        data_safety: None,
        docs: None,
    };
    record_info(
        backend,
        &info,
        IncidentSource::Api,
        IncidentLevel::Warning,
        session_id,
        None,
    )
    .await;
}

/// Extracts a session ID from an API path (`/api/v1/sessions/{id}/...`).
pub(crate) fn session_id_from_path(path: &str) -> Option<&str> {
    let mut segments = path.split('/');
    while let Some(segment) = segments.next() {
        if segment == "sessions" {
            return segments.next().filter(|value| !value.is_empty());
        }
    }
    None
}

/// Extracts the error code header attached by [`super::ApiError::into_response`].
pub(crate) fn error_code_from_headers(headers: &axum::http::HeaderMap) -> Option<ErrorCode> {
    let value = header_str(
        headers,
        axum::http::header::HeaderName::from_static("x-fxg-error-code"),
    )?;
    match value.parse::<ErrorCode>() {
        Ok(code) => Some(code),
        Err(err) => {
            tracing::debug!("invalid x-fxg-error-code header: {err}");
            None
        }
    }
}

/// Incidents whose status code is worth persisting (server errors and recoverable conflicts).
pub(crate) fn should_record_status(status: StatusCode) -> bool {
    status.is_server_error()
        || matches!(
            status,
            StatusCode::CONFLICT | StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
        )
}

/// Builds a redacted support bundle for the endpoint.
pub(crate) async fn build_support_bundle<B: ClientApiBackend>(
    backend: &B,
) -> Result<SupportBundle, super::ApiError> {
    let diagnostics = build_diagnostics(backend).await?;
    let counts = backend
        .db()
        .database_counts()
        .await
        .map_err(super::ApiError::from)?;
    let sessions = backend
        .db()
        .support_bundle_sessions(SUPPORT_BUNDLE_SESSION_LIMIT)
        .await
        .map_err(super::ApiError::from)?;
    let pending_outbox_events = match backend.connection_role() {
        ConnectionRole::LocalNode => backend
            .db()
            .unsynced_event_count()
            .await
            .map_err(super::ApiError::from)?,
        ConnectionRole::Hub => 0,
    };
    let home = backend.fxg_home();
    let database_file = database_file_path(backend);
    let database = DatabaseStats {
        path: database_file
            .as_ref()
            .map(|path| redact_text(&path.to_string_lossy(), home))
            .unwrap_or_default(),
        size_bytes: database_file
            .as_ref()
            .map(|path| fxg_db::incidents::file_size(path))
            .unwrap_or(0),
        sessions: counts.sessions,
        events: counts.events,
        incidents: counts.incidents,
        pending_outbox_events,
    };
    let config = redact_config(backend.support_bundle_config(), home);
    let logs = backend
        .support_bundle_logs(SUPPORT_BUNDLE_LOG_LINES)
        .into_iter()
        .map(|line| redact_text(&line, home))
        .collect();
    Ok(SupportBundle {
        generated_at: now_ms(),
        version: backend.version().to_owned(),
        role: backend.connection_role(),
        config,
        diagnostics: redact_diagnostics(diagnostics, home),
        database,
        sessions,
        logs,
    })
}

/// Builds the shared diagnostics response (used by `GET /api/v1/diagnostics`).
pub(crate) async fn build_diagnostics<B: ClientApiBackend>(
    backend: &B,
) -> Result<DiagnosticsResponse, super::ApiError> {
    let health = build_system_status(backend).await;
    let incidents = backend
        .db()
        .list_incidents(50, false)
        .await
        .map_err(super::ApiError::from)?;
    let agents = backend.agent_diagnostics();
    let node_log = backend.fxg_home().map(|home| {
        home.join("logs")
            .join(backend.log_file_name())
            .to_string_lossy()
            .into_owned()
    });
    Ok(DiagnosticsResponse {
        node_id: backend
            .local_node_id()
            .map(|node_id| node_id.as_str().to_owned())
            .unwrap_or_else(|| "central-server".to_owned()),
        generated_at: now_ms(),
        agents,
        node_log,
        health,
        incidents,
    })
}

fn database_file_path<B: ClientApiBackend>(backend: &B) -> Option<std::path::PathBuf> {
    let home = backend.fxg_home()?;
    let file = match backend.connection_role() {
        ConnectionRole::LocalNode => fxg_protocol::config::NODE_DB_FILE_NAME,
        ConnectionRole::Hub => fxg_protocol::config::HUB_DB_FILE_NAME,
    };
    Some(home.join(file))
}

fn redact_config(config: SupportBundleConfig, home: Option<&Path>) -> SupportBundleConfig {
    SupportBundleConfig {
        fxg_home: redact_text(&config.fxg_home, home),
        log_level: config.log_level.map(|value| redact_text(&value, home)),
        log_file: config.log_file.map(|value| redact_text(&value, home)),
        log_format: config.log_format.map(|value| redact_text(&value, home)),
        node_listen_addr: config.node_listen_addr,
        hub_url: config.hub_url.map(|value| redact_text(&value, home)),
        server_listen_addr: config.server_listen_addr,
        allowed_hosts: config.allowed_hosts,
        allowed_origins: config.allowed_origins,
    }
}

fn redact_diagnostics(
    mut diagnostics: DiagnosticsResponse,
    home: Option<&Path>,
) -> DiagnosticsResponse {
    diagnostics.node_log = diagnostics.node_log.map(|path| redact_text(&path, home));
    for agent in &mut diagnostics.agents {
        for line in &mut agent.stderr_tail {
            *line = redact_text(line, home);
        }
    }
    for component in &mut diagnostics.health.components {
        if let Some(message) = component.message.as_mut() {
            *message = redact_text(message, home);
        }
    }
    for incident in &mut diagnostics.incidents {
        incident.summary = redact_text(&incident.summary, home);
        incident.details = redact_json(&incident.details, home);
    }
    diagnostics
}

/// Redacts secrets and the home directory from free-form text.
pub(crate) fn redact_text(text: &str, home: Option<&Path>) -> String {
    let mut value = text.to_owned();
    if let Some(home) = home {
        let home = home.to_string_lossy().into_owned();
        if !home.is_empty() {
            value = value.replace(&home, "~");
        }
    }
    for (marker, end_chars) in [
        ("authorization:", "\r\n"),
        ("bearer ", " \t\r\n"),
        ("fxg_session=", "; \t\r\n\""),
        ("token=", "& \t\r\n\""),
        ("password=", "& \t\r\n\""),
        ("secret=", "& \t\r\n\""),
    ] {
        value = redact_after(&value, marker, end_chars);
    }
    redact_url_credentials(&value)
}

/// Replaces `scheme://user:pass@host` credentials with `scheme://<redacted>@host`.
fn redact_url_credentials(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(relative) = text[cursor..].find("://") {
        let scheme_end = cursor + relative + 3;
        let after = &text[scheme_end..];
        let host_end = after
            .find(['/', ' ', '\t', '\r', '\n', '?', '#'])
            .unwrap_or(after.len());
        let authority = &after[..host_end];
        match authority.rfind('@') {
            Some(at) => {
                result.push_str(&text[cursor..scheme_end]);
                result.push_str("<redacted>@");
                cursor = scheme_end + at + 1;
            }
            None => {
                let end = scheme_end + host_end;
                result.push_str(&text[cursor..end]);
                cursor = end;
            }
        }
    }
    result.push_str(&text[cursor..]);
    result
}

/// Replaces the value following every `marker` with `<redacted>`.
fn redact_after(text: &str, marker: &str, end_chars: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find(marker) {
        let marker_end = cursor + relative + marker.len();
        result.push_str(&text[cursor..marker_end]);
        result.push_str("<redacted>");
        let after = &text[marker_end..];
        let value_end = after
            .find(|ch| end_chars.contains(ch))
            .unwrap_or(after.len());
        cursor = marker_end + value_end;
    }
    result.push_str(&text[cursor..]);
    result
}

/// Redacts secret-looking object keys and the home directory inside JSON values.
pub(crate) fn redact_json(value: &RawJson, home: Option<&Path>) -> RawJson {
    RawJson::new(redact_json_value(value.as_value(), home))
}

fn redact_json_value(value: &serde_json::Value, home: Option<&Path>) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::Value::String(redact_text(text, home)),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| redact_json_value(item, home))
                .collect(),
        ),
        serde_json::Value::Object(map) => {
            let mut redacted = serde_json::Map::with_capacity(map.len());
            for (key, item) in map {
                if is_secret_key(key) {
                    redacted.insert(
                        key.clone(),
                        serde_json::Value::String("<redacted>".to_owned()),
                    );
                } else {
                    redacted.insert(key.clone(), redact_json_value(item, home));
                }
            }
            serde_json::Value::Object(redacted)
        }
        other => other.clone(),
    }
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("token")
        || key.contains("password")
        || key.contains("secret")
        || key.contains("authorization")
        || key.contains("cookie")
}

/// Reads the tail of a log file (oldest first). Missing files yield an empty tail.
pub fn log_tail(path: &Path, lines: usize) -> Vec<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!(path = %path.display(), "failed to read the log file: {err}");
            return Vec::new();
        }
    };
    let mut tail: Vec<String> = text.lines().rev().take(lines).map(str::to_owned).collect();
    tail.reverse();
    tail
}

/// Retention metadata exposed by `GET /api/v1/diagnostics/incidents`.
pub const fn incident_retention() -> (u32, u32) {
    (INCIDENT_RETENTION_DAYS, INCIDENT_MAX_ENTRIES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn redacts_tokens_and_cookies() {
        let text = "GET /?token=abc123 Authorization: Bearer xyz Cookie: fxg_session=secret; a=b";
        let redacted = redact_text(text, None);
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("xyz"));
        assert!(!redacted.contains("secret"));
        assert!(redacted.contains("<redacted>"));
    }

    #[test]
    fn redacts_home_directory() {
        let text = "/home/alice/.flexagent/logs/node.log";
        let redacted = redact_text(text, Some(Path::new("/home/alice")));
        assert_eq!(redacted, "~/.flexagent/logs/node.log");
    }

    #[test]
    fn redacts_url_credentials() {
        let text = "wss://user:pass@example.com/api/v1/node/ws";
        assert_eq!(
            redact_text(text, None),
            "wss://<redacted>@example.com/api/v1/node/ws"
        );
        assert_eq!(
            redact_text("https://example.com/path", None),
            "https://example.com/path"
        );
    }

    #[test]
    fn redacts_secret_json_keys_recursively() {
        let json = RawJson::new(serde_json::json!({
            "token": "abc",
            "nested": { "password": "pw", "path": "/home/alice/work" }
        }));
        let redacted = redact_json(&json, Some(Path::new("/home/alice")));
        let value = redacted.as_value();
        assert_eq!(value["token"], "<redacted>");
        assert_eq!(value["nested"]["password"], "<redacted>");
        assert_eq!(value["nested"]["path"], "~/work");
    }

    #[test]
    fn extracts_session_id_from_path() {
        assert_eq!(
            session_id_from_path("/api/v1/sessions/ses_1/permissions/x/respond"),
            Some("ses_1")
        );
        assert_eq!(session_id_from_path("/api/v1/projects"), None);
    }

    #[test]
    fn records_only_server_errors_and_recoverable_conflicts() {
        assert!(should_record_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(should_record_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(should_record_status(StatusCode::CONFLICT));
        assert!(!should_record_status(StatusCode::BAD_REQUEST));
        assert!(!should_record_status(StatusCode::NOT_FOUND));
        assert!(!should_record_status(StatusCode::UNAUTHORIZED));
    }
}
