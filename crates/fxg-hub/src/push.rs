//! VAPID Web Push delivery (hub).
//!
//! Design: `docs/03-protocol-and-api.md` §3.1, `docs/05-cli-and-pwa-ui.md` §3.3.
//!
//! - The VAPID key pair is generated on first startup and stored in `~/.flexagent/vapid.json`
//!   (the public key is distributed via `vapid_public_key` in `GET /api/v1/system/info`).
//! - When an approval request (a `PermissionRequest` event) is applied, a notification is
//!   fanned out to every registered subscription. For Approve / Reject on the Android
//!   notification banner, the Service Worker calls `POST .../permissions/:req_id/respond`.
//! - Encryption (RFC 8291 aes128gcm) and VAPID signing (RFC 8292) use pure Rust
//!   [`web_push_native`] (`p256` / `aes-gcm` / `hkdf`) and do not depend on OpenSSL. HTTP
//!   delivery uses the workspace `reqwest`.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use fxg_db::{Db, PushSubscriptionRecord};
use fxg_protocol::{ClientId, SessionId};
use serde::{Deserialize, Serialize};
use web_push_native::jwt_simple::algorithms::ES256KeyPair;
use web_push_native::p256::elliptic_curve::sec1::ToEncodedPoint;
use web_push_native::{Auth, WebPushBuilder};

use crate::presence::PresenceRegistry;

/// VAPID key file name (`~/.flexagent/vapid.json`).
pub const VAPID_KEYS_FILE: &str = "vapid.json";

/// VAPID `sub` claim (contact).
const VAPID_CONTACT: &str = "mailto:flexagent@localhost";

/// VAPID key pair (base64url / no padding).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VapidKeys {
    /// P-256 private key (base64url of the 32-byte scalar)
    private_key: String,
    /// P-256 public key (base64url of the 65-byte uncompressed point)
    public_key: String,
}

/// Web Push delivery error.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// I/O error on the VAPID key file
    #[error("vapid key io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON error for VAPID keys / payload
    #[error("push json error: {0}")]
    Json(#[from] serde_json::Error),
    /// P-256 key generation failed (no random value was a valid scalar)
    #[error("failed to generate a vapid keypair")]
    KeyGeneration,
    /// The push endpoint URL is invalid
    #[error("invalid push endpoint: {0}")]
    InvalidEndpoint(String),
    /// The subscription keys (`p256dh` / `auth`) are invalid
    #[error("invalid push subscription keys")]
    InvalidKeys,
    /// Signing / encryption error (web-push-native)
    #[error("web push build error: {0}")]
    Build(#[from] web_push_native::Error),
    /// HTTP delivery error
    #[error("push request error: {0}")]
    Http(#[from] reqwest::Error),
    /// The push service returned an error status
    #[error("push endpoint returned status {0}")]
    HttpStatus(u16),
}

/// Notification payload (interpreted by the Service Worker `push` handler).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushNotificationPayload {
    /// Notification title (e.g. `[flexagent] Approval request (Home-Win)`)
    pub title: String,
    /// Notification body (e.g. `opencode: Run "cargo test --workspace"`)
    pub body: String,
    /// Target session ID
    pub session_id: String,
    /// Approval request ID
    pub request_id: String,
    /// Replacement tag for the notification (overwrites notifications for the same request)
    pub tag: String,
    /// `option_id` sent by the `[Approve]` button
    pub allow_option_id: Option<String>,
    /// `option_id` sent by the `[Reject]` button
    pub reject_option_id: Option<String>,
}

/// Delivery outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushSendOutcome {
    /// Delivered successfully
    Sent,
    /// The subscription is no longer valid (`404` / `410`; delete the subscription)
    Gone,
}

/// Service that holds the VAPID keys and sends Web Push messages.
#[derive(Clone)]
pub struct PushService {
    client: reqwest::Client,
    key_pair: Arc<ES256KeyPair>,
    public_key: String,
    subject: Arc<std::sync::RwLock<String>>,
}

impl PushService {
    /// Loads `~/.flexagent/vapid.json` (generating and saving it if absent).
    pub fn load_or_create(fxg_home: &Path) -> Result<Self, PushError> {
        let path = fxg_home.join(VAPID_KEYS_FILE);
        let keys = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str::<VapidKeys>(&raw)?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let keys = generate_vapid_keys()?;
                std::fs::create_dir_all(fxg_home)?;
                std::fs::write(&path, serde_json::to_string_pretty(&keys)?)?;
                set_owner_only_permissions(&path)?;
                tracing::info!(path = %path.display(), "generated vapid keypair");
                keys
            }
            Err(err) => return Err(err.into()),
        };
        if let Err(err) = set_owner_only_permissions(&path) {
            tracing::warn!(path = %path.display(), "failed to restrict vapid key permissions: {err}");
        }
        let private_bytes = decode_base64(&keys.private_key).ok_or(PushError::InvalidKeys)?;
        let key_pair = match ES256KeyPair::from_bytes(&private_bytes) {
            Ok(key_pair) => key_pair,
            Err(err) => {
                tracing::warn!("failed to load the VAPID key pair: {err}");
                return Err(PushError::InvalidKeys);
            }
        };
        Ok(Self {
            client: reqwest::Client::new(),
            key_pair: Arc::new(key_pair),
            public_key: keys.public_key,
            subject: Arc::new(std::sync::RwLock::new(VAPID_CONTACT.to_owned())),
        })
    }

    /// Replaces the VAPID `sub` claim (contact) after a settings update.
    pub fn set_subject(&self, subject: Option<&str>) {
        let value = subject
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(VAPID_CONTACT);
        *self.subject.write().expect("vapid subject lock poisoned") = value.to_owned();
    }

    /// VAPID public key (base64url) distributed via `GET /api/v1/system/info`.
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// Builds the HTTP push request (already encrypted and VAPID-signed) for one subscription.
    fn build_request(
        &self,
        subscription: &PushSubscriptionRecord,
        body: Vec<u8>,
    ) -> Result<http::Request<Vec<u8>>, PushError> {
        let endpoint: http::Uri = match subscription.endpoint.parse() {
            Ok(endpoint) => endpoint,
            Err(err) => {
                tracing::debug!(
                    endpoint = %subscription.endpoint,
                    "push subscription endpoint is not a valid URI: {err}"
                );
                return Err(PushError::InvalidEndpoint(subscription.endpoint.clone()));
            }
        };
        let ua_public_bytes = decode_base64(&subscription.p256dh).ok_or(PushError::InvalidKeys)?;
        let ua_public = match web_push_native::p256::PublicKey::from_sec1_bytes(&ua_public_bytes) {
            Ok(key) => key,
            Err(err) => {
                tracing::debug!("push subscription p256dh key is invalid: {err}");
                return Err(PushError::InvalidKeys);
            }
        };
        let auth_bytes = decode_base64(&subscription.auth).ok_or(PushError::InvalidKeys)?;
        if auth_bytes.len() != 16 {
            return Err(PushError::InvalidKeys);
        }
        let auth = Auth::clone_from_slice(&auth_bytes);
        let subject = self
            .subject
            .read()
            .expect("vapid subject lock poisoned")
            .clone();
        let builder =
            WebPushBuilder::new(endpoint, ua_public, auth).with_vapid(&self.key_pair, &subject);
        Ok(builder.build(body)?)
    }

    /// Sends a notification to one subscription.
    pub async fn send(
        &self,
        subscription: &PushSubscriptionRecord,
        payload: &PushNotificationPayload,
    ) -> Result<PushSendOutcome, PushError> {
        let body = serde_json::to_vec(payload)?;
        let request = self.build_request(subscription, body)?;

        // Send the `http::Request` via reqwest
        let (parts, body) = request.into_parts();
        let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
            .unwrap_or(reqwest::Method::POST);
        let mut builder = self
            .client
            .request(method, parts.uri.to_string())
            .body(body);
        for (name, value) in parts.headers.iter() {
            builder = builder.header(name.as_str(), value.as_bytes());
        }
        let response = builder.send().await?;
        let status = response.status().as_u16();
        match status {
            200..=299 => Ok(PushSendOutcome::Sent),
            404 | 410 => Ok(PushSendOutcome::Gone),
            other => Err(PushError::HttpStatus(other)),
        }
    }
}

/// Generates a P-256 key pair (the public key is stored in uncompressed SEC1 form).
fn generate_vapid_keys() -> Result<VapidKeys, PushError> {
    for _ in 0..32 {
        let scalar: [u8; 32] = rand::random();
        let Ok(secret) = web_push_native::p256::SecretKey::from_slice(&scalar) else {
            continue;
        };
        let private_key = URL_SAFE_NO_PAD.encode(secret.to_bytes());
        let public_key =
            URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes());
        return Ok(VapidKeys {
            private_key,
            public_key,
        });
    }
    Err(PushError::KeyGeneration)
}

/// Decodes base64url (no padding) or standard base64.
fn decode_base64(value: &str) -> Option<Vec<u8>> {
    match URL_SAFE_NO_PAD.decode(value) {
        Ok(bytes) => Some(bytes),
        Err(url_err) => match STANDARD.decode(value) {
            Ok(bytes) => Some(bytes),
            Err(standard_err) => {
                tracing::debug!("failed to decode base64 ({url_err}; {standard_err})");
                None
            }
        },
    }
}

/// Builds a notification payload from a `PermissionRequest` event.
pub fn permission_notification(
    session_id: &str,
    session_title: Option<&str>,
    node_label: &str,
    request_id: &str,
    tool_name: &str,
    summary: &str,
    options: &[fxg_protocol::common::PermissionOption],
) -> PushNotificationPayload {
    let allow = options
        .iter()
        .find(|option| option.kind.is_allow())
        .map(|option| option.option_id.clone());
    let reject = options
        .iter()
        .find(|option| option.kind.is_reject())
        .map(|option| option.option_id.clone());
    let title = match session_title {
        Some(title) => format!("[flexagent] Approval request ({node_label}) — {title}"),
        None => format!("[flexagent] Approval request ({node_label})"),
    };
    PushNotificationPayload {
        title,
        body: format!("{tool_name}: {summary}"),
        session_id: session_id.to_owned(),
        request_id: request_id.to_owned(),
        tag: format!("fxg-permission-{request_id}"),
        allow_option_id: allow,
        reject_option_id: reject,
    }
}

/// Builds a notification payload from an `ElicitationRequest` event.
///
/// Form answers can only be given inside the app, so instead of the direct response buttons
/// used for approvals, the only path is tapping the notification to open the app.
pub fn elicitation_notification(
    session_id: &str,
    session_title: Option<&str>,
    node_label: &str,
    elicitation_id: &str,
    message: &str,
) -> PushNotificationPayload {
    let title = match session_title {
        Some(title) => format!("[flexagent] Question ({node_label}) — {title}"),
        None => format!("[flexagent] Question ({node_label})"),
    };
    PushNotificationPayload {
        title,
        body: message.to_owned(),
        session_id: session_id.to_owned(),
        request_id: elicitation_id.to_owned(),
        tag: format!("fxg-elicitation-{elicitation_id}"),
        allow_option_id: None,
        reject_option_id: None,
    }
}

/// Builds a notification payload when a turn finishes (normal completion or error).
pub fn turn_finished_notification(
    session_id: &str,
    session_title: Option<&str>,
    node_label: &str,
    failed: bool,
) -> PushNotificationPayload {
    let title = match session_title {
        Some(title) => format!("[flexagent] Turn finished ({node_label}) — {title}"),
        None => format!("[flexagent] Turn finished ({node_label})"),
    };
    PushNotificationPayload {
        title,
        body: if failed {
            "The turn ended with an error. Tap to open the session.".to_owned()
        } else {
            "The agent finished the turn. Tap to open the session.".to_owned()
        },
        session_id: session_id.to_owned(),
        request_id: session_id.to_owned(),
        tag: format!("fxg-turn-{session_id}"),
        allow_option_id: None,
        reject_option_id: None,
    }
}

/// How Web Push chooses target devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "Phase 3 reads the policy from hub settings")]
pub(crate) enum TargetPolicy {
    /// Every subscription.
    All,
    /// Clients with a visible, focused tab (fallback applies when none).
    ActiveOnly,
    /// Active clients plus clients that were active within the recent window.
    ActiveOrRecent,
    /// The client that was active most recently.
    MostRecent,
}

/// Notification delivery targeting.
///
/// These are defaults for Phase 2; hub settings (Phase 3) will provide the values.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NotificationTargeting {
    /// Target policy
    pub policy: TargetPolicy,
    /// Window for [`TargetPolicy::ActiveOrRecent`]
    pub recent_window: Duration,
    /// Send to every subscription when the policy has no target
    pub fallback_all: bool,
    /// Do not notify a client that currently has the target session open
    pub suppress_focused_session: bool,
}

impl Default for NotificationTargeting {
    fn default() -> Self {
        Self {
            policy: TargetPolicy::ActiveOnly,
            recent_window: Duration::from_secs(600),
            fallback_all: true,
            suppress_focused_session: true,
        }
    }
}

/// Resolves the client IDs to notify, or `None` when every subscription is eligible.
fn target_clients(
    presence: &PresenceRegistry,
    targeting: &NotificationTargeting,
    now: Instant,
) -> Option<HashSet<ClientId>> {
    let clients = match targeting.policy {
        TargetPolicy::All => return None,
        TargetPolicy::ActiveOnly => presence.active_clients_at(now),
        TargetPolicy::ActiveOrRecent => presence.recent_clients_at(now, targeting.recent_window),
        TargetPolicy::MostRecent => presence.most_recent_client_at(now).into_iter().collect(),
    };
    if clients.is_empty() {
        return if targeting.fallback_all {
            None
        } else {
            Some(HashSet::new())
        };
    }
    Some(clients)
}

/// Selects the subscriptions that should receive a notification for `session_id`.
///
/// Subscriptions registered before `client_id` existed are only eligible when every
/// subscription is targeted (policy `All` or the no-active-client fallback).
fn select_subscriptions<'a>(
    subscriptions: &'a [PushSubscriptionRecord],
    presence: &PresenceRegistry,
    session_id: &SessionId,
    targeting: &NotificationTargeting,
    now: Instant,
) -> Vec<&'a PushSubscriptionRecord> {
    let focused = if targeting.suppress_focused_session {
        presence.clients_focused_on_at(session_id, now)
    } else {
        HashSet::new()
    };
    let clients = target_clients(presence, targeting, now);
    subscriptions
        .iter()
        .filter(|subscription| {
            if subscription
                .client_id
                .as_ref()
                .is_some_and(|client_id| focused.contains(client_id))
            {
                return false;
            }
            match &clients {
                None => true,
                Some(clients) => subscription
                    .client_id
                    .as_ref()
                    .is_some_and(|client_id| clients.contains(client_id)),
            }
        })
        .collect()
}

/// Selects the subscriptions for one session at the current time.
fn targets_for_session<'a>(
    subscriptions: &'a [PushSubscriptionRecord],
    presence: &PresenceRegistry,
    session_id: &str,
    targeting: &NotificationTargeting,
) -> Vec<&'a PushSubscriptionRecord> {
    select_subscriptions(
        subscriptions,
        presence,
        &SessionId::new(session_id),
        targeting,
        Instant::now(),
    )
}

/// Tracks sessions whose current turn was observed as `Running`.
///
/// Live sync delivers one event per `EventBatchPush`, so the `Running` and `Idle` statuses of a
/// turn normally arrive in separate `apply_event_batch` calls. Keeping the set in hub state (and
/// seeding it from `hub.db` at startup) makes the Running → Idle transition observable across
/// batches and hub restarts.
#[derive(Debug, Default)]
pub(crate) struct RunningTurns {
    sessions: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl RunningTurns {
    /// Seeds the tracker with sessions that are already running at startup.
    pub(crate) fn from_session_ids(ids: impl IntoIterator<Item = String>) -> Self {
        Self {
            sessions: std::sync::Mutex::new(ids.into_iter().collect()),
        }
    }

    /// Records that a turn started.
    fn mark_running(&self, session_id: &str) {
        self.sessions
            .lock()
            .expect("running turns lock poisoned")
            .insert(session_id.to_owned());
    }

    /// Records that a turn ended and returns whether it had been observed as running.
    fn finish(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .expect("running turns lock poisoned")
            .remove(session_id)
    }
}

/// Selects the newly inserted events that should produce a Web Push notification.
///
/// A turn-finished notification is selected only when a `Running` status was observed first
/// (including across separate batches). `resync` batches still update the running set so a later
/// live transition is recognized, but never notify: replaying history must not wake devices.
pub(crate) fn notification_events(
    events: &[fxg_protocol::events::SessionEventEnvelope],
    running: &RunningTurns,
    resync: bool,
) -> Vec<fxg_protocol::events::SessionEventEnvelope> {
    use fxg_protocol::common::SessionStatus;
    use fxg_protocol::events::UnifiedEventPayload;

    let mut notify = Vec::new();
    for event in events {
        match &event.payload {
            UnifiedEventPayload::PermissionRequest { .. }
            | UnifiedEventPayload::ElicitationRequest { .. } => {
                if !resync {
                    notify.push(event.clone());
                }
            }
            UnifiedEventPayload::StatusChanged { status, .. } => match status {
                SessionStatus::Running => running.mark_running(event.session_id.as_str()),
                SessionStatus::Idle | SessionStatus::Error => {
                    let finished = running.finish(event.session_id.as_str());
                    if finished && !resync {
                        notify.push(event.clone());
                    }
                }
                // A stopped session no longer has a running turn; do not notify (the stop is
                // user-initiated) but drop the stale entry.
                SessionStatus::Stopped => {
                    running.finish(event.session_id.as_str());
                }
                _ => {}
            },
            UnifiedEventPayload::SessionDeleted {} => {
                running.finish(event.session_id.as_str());
            }
            _ => {}
        }
    }
    notify
}

/// Sends one payload to every subscription (expired subscriptions are deleted). Failures only
/// produce a warning log.
async fn send_to_subscriptions(
    push: &PushService,
    db: &Db,
    subscriptions: &[&PushSubscriptionRecord],
    payload: &PushNotificationPayload,
    counters: Option<&crate::api::DiagnosticsCounters>,
) {
    for subscription in subscriptions.iter().copied() {
        match push.send(subscription, payload).await {
            Ok(PushSendOutcome::Sent) => {
                tracing::debug!(endpoint = %subscription.endpoint, request_id = %payload.request_id, "push sent");
            }
            Ok(PushSendOutcome::Gone) => {
                tracing::info!(endpoint = %subscription.endpoint, "removing expired push subscription");
                if let Err(err) = db.delete_push_subscription(&subscription.endpoint).await {
                    tracing::warn!("failed to delete push subscription: {err}");
                }
            }
            Err(err) => {
                if let Some(counters) = counters {
                    counters.incr(crate::api::CounterKind::PushFailures);
                }
                tracing::warn!(endpoint = %subscription.endpoint, "push send failed: {err}");
            }
        }
    }
}

/// Notifies every subscription about `PermissionRequest`s in an event batch.
///
/// Expired subscriptions (`404` / `410`) are deleted. Failures only produce a warning log and
/// processing continues.
pub(crate) async fn notify_permission_requests(
    push: &PushService,
    db: &Db,
    presence: &PresenceRegistry,
    targeting: &NotificationTargeting,
    events: &[fxg_protocol::events::SessionEventEnvelope],
    counters: Option<&crate::api::DiagnosticsCounters>,
) {
    use fxg_protocol::events::UnifiedEventPayload;

    let requests: Vec<(
        String,
        String,
        String,
        String,
        Vec<fxg_protocol::common::PermissionOption>,
    )> = events
        .iter()
        .filter_map(|event| match &event.payload {
            UnifiedEventPayload::PermissionRequest {
                request_id,
                tool_name,
                summary,
                options,
                ..
            } => Some((
                event.session_id.to_string(),
                request_id.clone(),
                tool_name.clone(),
                summary.clone(),
                options.clone(),
            )),
            _ => None,
        })
        .collect();
    if requests.is_empty() {
        return;
    }

    let subscriptions = match db.list_push_subscriptions().await {
        Ok(subscriptions) => subscriptions,
        Err(err) => {
            tracing::warn!("failed to load push subscriptions: {err}");
            return;
        }
    };
    if subscriptions.is_empty() {
        return;
    }

    for (session_id, request_id, tool_name, summary, options) in requests {
        let session = match db.get_session(&session_id).await {
            Ok(session) => session,
            Err(err) => {
                tracing::warn!(session_id, "failed to load session for push: {err}");
                None
            }
        };
        let node_label = session
            .as_ref()
            .map(|session| session.node_id.as_str().to_owned())
            .unwrap_or_else(|| "unknown".to_owned());
        let payload = permission_notification(
            &session_id,
            session.as_ref().map(|session| session.title.as_str()),
            &node_label,
            &request_id,
            &tool_name,
            &summary,
            &options,
        );

        let selected = targets_for_session(&subscriptions, presence, &session_id, targeting);
        if selected.is_empty() {
            continue;
        }
        send_to_subscriptions(push, db, &selected, &payload, counters).await;
    }
}

/// Notifies every subscription about `ElicitationRequest`s in an event batch.
///
/// Expired subscriptions (`404` / `410`) are deleted. Failures only produce a warning log and
/// processing continues.
pub(crate) async fn notify_elicitations(
    push: &PushService,
    db: &Db,
    presence: &PresenceRegistry,
    targeting: &NotificationTargeting,
    events: &[fxg_protocol::events::SessionEventEnvelope],
    counters: Option<&crate::api::DiagnosticsCounters>,
) {
    use fxg_protocol::events::UnifiedEventPayload;

    let requests: Vec<(String, String, String)> = events
        .iter()
        .filter_map(|event| match &event.payload {
            UnifiedEventPayload::ElicitationRequest {
                elicitation_id,
                message,
                ..
            } => Some((
                event.session_id.to_string(),
                elicitation_id.clone(),
                message.clone(),
            )),
            _ => None,
        })
        .collect();
    if requests.is_empty() {
        return;
    }

    let subscriptions = match db.list_push_subscriptions().await {
        Ok(subscriptions) => subscriptions,
        Err(err) => {
            tracing::warn!("failed to load push subscriptions: {err}");
            return;
        }
    };
    if subscriptions.is_empty() {
        return;
    }

    for (session_id, elicitation_id, message) in requests {
        let session = match db.get_session(&session_id).await {
            Ok(session) => session,
            Err(err) => {
                tracing::warn!(session_id, "failed to load session for push: {err}");
                None
            }
        };
        let node_label = session
            .as_ref()
            .map(|session| session.node_id.as_str().to_owned())
            .unwrap_or_else(|| "unknown".to_owned());
        let payload = elicitation_notification(
            &session_id,
            session.as_ref().map(|session| session.title.as_str()),
            &node_label,
            &elicitation_id,
            &message,
        );

        let selected = targets_for_session(&subscriptions, presence, &session_id, targeting);
        if selected.is_empty() {
            continue;
        }
        send_to_subscriptions(push, db, &selected, &payload, counters).await;
    }
}

/// Sends a completion notification for finished turns.
pub(crate) async fn notify_turn_finished(
    push: &PushService,
    db: &Db,
    presence: &PresenceRegistry,
    targeting: &NotificationTargeting,
    events: &[fxg_protocol::events::SessionEventEnvelope],
    counters: Option<&crate::api::DiagnosticsCounters>,
) {
    use fxg_protocol::common::SessionStatus;
    use fxg_protocol::events::UnifiedEventPayload;

    let finished: Vec<(String, bool)> = events
        .iter()
        .filter_map(|event| match &event.payload {
            UnifiedEventPayload::StatusChanged { status, .. }
                if matches!(status, SessionStatus::Idle | SessionStatus::Error) =>
            {
                Some((
                    event.session_id.to_string(),
                    *status == SessionStatus::Error,
                ))
            }
            _ => None,
        })
        .collect();
    if finished.is_empty() {
        return;
    }
    let subscriptions = match db.list_push_subscriptions().await {
        Ok(subscriptions) => subscriptions,
        Err(err) => {
            tracing::warn!("failed to load push subscriptions: {err}");
            return;
        }
    };
    if subscriptions.is_empty() {
        return;
    }
    for (session_id, failed) in finished {
        let session = match db.get_session(&session_id).await {
            Ok(session) => session,
            Err(err) => {
                tracing::warn!(session_id, "failed to load session for push: {err}");
                continue;
            }
        };
        let Some(session) = session else {
            continue;
        };
        let node_label = session.node_id.to_string();
        let payload =
            turn_finished_notification(&session_id, Some(&session.title), &node_label, failed);
        let selected = targets_for_session(&subscriptions, presence, &session_id, targeting);
        if selected.is_empty() {
            continue;
        }
        send_to_subscriptions(push, db, &selected, &payload, counters).await;
    }
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> std::io::Result<()> {
    // On Windows, follow the ACLs of the user profile directory.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::common::{PermissionOption, PermissionOptionKind};

    fn test_service() -> (PushService, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PushService::load_or_create(dir.path()).expect("push service");
        (service, dir)
    }

    /// Generates a test UA (browser) side subscription.
    fn test_subscription(
        endpoint: &str,
    ) -> (
        PushSubscriptionRecord,
        web_push_native::p256::SecretKey,
        Auth,
    ) {
        let secret = loop {
            let scalar: [u8; 32] = rand::random();
            if let Ok(secret) = web_push_native::p256::SecretKey::from_slice(&scalar) {
                break secret;
            }
        };
        let public = secret.public_key().to_encoded_point(false);
        let auth_bytes: [u8; 16] = rand::random();
        let auth = Auth::clone_from_slice(&auth_bytes);
        let record = PushSubscriptionRecord {
            endpoint: endpoint.to_owned(),
            p256dh: URL_SAFE_NO_PAD.encode(public.as_bytes()),
            auth: URL_SAFE_NO_PAD.encode(auth_bytes),
            device_name: Some("test device".to_owned()),
            client_id: None,
        };
        (record, secret, auth)
    }

    #[test]
    fn vapid_keys_are_persisted_and_reloaded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = PushService::load_or_create(dir.path()).expect("generate");
        assert!(!first.public_key().is_empty());
        assert!(dir.path().join(VAPID_KEYS_FILE).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(VAPID_KEYS_FILE))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        let second = PushService::load_or_create(dir.path()).expect("reload");
        assert_eq!(first.public_key(), second.public_key());
    }

    #[test]
    fn push_request_is_encrypted_and_signed() {
        let (service, _dir) = test_service();
        let (subscription, ua_secret, auth) = test_subscription("https://push.example.com/v1/abc");
        let payload = PushNotificationPayload {
            title: "[flexagent] Approval request (Home-Win)".to_owned(),
            body: "opencode2: Run \"cargo test\"".to_owned(),
            session_id: "session-1".to_owned(),
            request_id: "req-1".to_owned(),
            tag: "fxg-permission-req-1".to_owned(),
            allow_option_id: Some("allow_once".to_owned()),
            reject_option_id: Some("reject".to_owned()),
        };
        let body = serde_json::to_vec(&payload).expect("json");
        let request = service
            .build_request(&subscription, body)
            .expect("build request");

        // The VAPID signature header is attached
        let authorization = match request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .expect("authorization header")
            .to_str()
        {
            Ok(value) => value,
            Err(_) => panic!("authorization header is not ASCII"),
        };
        assert!(authorization.starts_with("vapid t="), "{authorization}");
        assert!(authorization.contains("k="), "{authorization}");
        assert!(request.headers().contains_key("content-encoding"));

        // The UA, the only recipient, can decrypt it (RFC 8291 round-trip check)
        let body = request.into_body();
        let decrypted = web_push_native::decrypt(body, &ua_secret, &auth).expect("decrypt");
        let decoded: PushNotificationPayload =
            serde_json::from_slice(&decrypted).expect("payload json");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn permission_notification_maps_allow_and_reject_options() {
        let options = vec![
            PermissionOption {
                option_id: "reject".to_owned(),
                name: "Reject".to_owned(),
                kind: PermissionOptionKind::RejectOnce,
            },
            PermissionOption {
                option_id: "allow_once".to_owned(),
                name: "Allow".to_owned(),
                kind: PermissionOptionKind::AllowOnce,
            },
            PermissionOption {
                option_id: "allow_always".to_owned(),
                name: "Always".to_owned(),
                kind: PermissionOptionKind::AllowAlways,
            },
        ];
        let payload = permission_notification(
            "session-1",
            Some("Fix auth"),
            "Home-Win",
            "req-1",
            "terminal/create",
            "Run cargo test",
            &options,
        );
        assert_eq!(
            payload.title,
            "[flexagent] Approval request (Home-Win) — Fix auth"
        );
        assert_eq!(payload.body, "terminal/create: Run cargo test");
        assert_eq!(payload.tag, "fxg-permission-req-1");
        assert_eq!(payload.allow_option_id.as_deref(), Some("allow_once"));
        assert_eq!(payload.reject_option_id.as_deref(), Some("reject"));
    }

    #[test]
    fn elicitation_notification_has_no_direct_answer_buttons() {
        let payload = elicitation_notification(
            "session-1",
            Some("Fix auth"),
            "Home-Win",
            "elic-1",
            "Which strategy should we use?",
        );
        assert_eq!(payload.title, "[flexagent] Question (Home-Win) — Fix auth");
        assert_eq!(payload.body, "Which strategy should we use?");
        assert_eq!(payload.tag, "fxg-elicitation-elic-1");
        // Form answers are in-app only (no direct replies from the notification)
        assert!(payload.allow_option_id.is_none());
        assert!(payload.reject_option_id.is_none());
    }

    #[test]
    fn turn_finished_notification_has_no_action_buttons() {
        let payload = turn_finished_notification("session-1", Some("Fix auth"), "Home-Win", false);
        assert_eq!(
            payload.title,
            "[flexagent] Turn finished (Home-Win) — Fix auth"
        );
        assert_eq!(payload.tag, "fxg-turn-session-1");
        assert!(payload.allow_option_id.is_none());
        assert!(payload.reject_option_id.is_none());
    }

    fn event(
        session_id: &str,
        node_seq: u64,
        payload: fxg_protocol::events::UnifiedEventPayload,
    ) -> fxg_protocol::events::SessionEventEnvelope {
        fxg_protocol::events::SessionEventEnvelope {
            event_id: fxg_protocol::EventId::new(format!("event-{node_seq}")),
            session_id: fxg_protocol::SessionId::new(session_id),
            node_seq,
            created_at: 0,
            payload,
        }
    }

    fn status_event(
        session_id: &str,
        node_seq: u64,
        status: fxg_protocol::common::SessionStatus,
    ) -> fxg_protocol::events::SessionEventEnvelope {
        event(
            session_id,
            node_seq,
            fxg_protocol::events::UnifiedEventPayload::StatusChanged {
                status,
                error_message: None,
            },
        )
    }

    fn permission_event(
        session_id: &str,
        node_seq: u64,
    ) -> fxg_protocol::events::SessionEventEnvelope {
        event(
            session_id,
            node_seq,
            fxg_protocol::events::UnifiedEventPayload::PermissionRequest {
                request_id: "req-1".to_owned(),
                tool_name: "terminal/create".to_owned(),
                summary: "run cargo test".to_owned(),
                options: Vec::new(),
                details: fxg_protocol::common::RawJson::default(),
                subagent_id: None,
            },
        )
    }

    fn elicitation_event(
        session_id: &str,
        node_seq: u64,
    ) -> fxg_protocol::events::SessionEventEnvelope {
        event(
            session_id,
            node_seq,
            fxg_protocol::events::UnifiedEventPayload::ElicitationRequest {
                elicitation_id: "elic-1".to_owned(),
                message: "Which strategy?".to_owned(),
                mode: fxg_protocol::common::ElicitationMode::Form,
                requested_schema: fxg_protocol::common::ElicitationSchema {
                    schema_type: fxg_protocol::common::ElicitationSchemaType::Object,
                    title: None,
                    description: None,
                    properties: Default::default(),
                    required: None,
                    extra: Default::default(),
                },
                tool_call_id: None,
                subagent_id: None,
            },
        )
    }

    #[test]
    fn turn_finished_requires_running_observed_in_a_previous_batch() {
        use fxg_protocol::common::SessionStatus;

        let running = RunningTurns::default();
        // Live sync sends one event per batch, so Running and Idle arrive separately.
        assert!(
            notification_events(
                &[status_event("session-1", 1, SessionStatus::Running)],
                &running,
                false
            )
            .is_empty()
        );
        let finished = notification_events(
            &[status_event("session-1", 2, SessionStatus::Idle)],
            &running,
            false,
        );
        assert_eq!(finished.len(), 1);
        // A second Idle without a new turn must not notify again.
        assert!(
            notification_events(
                &[status_event("session-1", 3, SessionStatus::Idle)],
                &running,
                false
            )
            .is_empty()
        );
    }

    #[test]
    fn turn_finished_uses_state_seeded_from_the_database() {
        use fxg_protocol::common::SessionStatus;

        // A hub restart seeds the set from running sessions in hub.db.
        let running = RunningTurns::from_session_ids(["session-1".to_owned()]);
        let finished = notification_events(
            &[status_event("session-1", 2, SessionStatus::Error)],
            &running,
            false,
        );
        assert_eq!(finished.len(), 1);
    }

    #[test]
    fn resync_updates_running_state_without_notifying() {
        use fxg_protocol::common::SessionStatus;

        let running = RunningTurns::default();
        assert!(
            notification_events(
                &[status_event("session-1", 1, SessionStatus::Running)],
                &running,
                true
            )
            .is_empty()
        );
        assert!(
            notification_events(
                &[status_event("session-1", 2, SessionStatus::Idle)],
                &running,
                true
            )
            .is_empty()
        );
        // The resync already consumed the transition; no stale completion is emitted.
        assert!(
            notification_events(
                &[status_event("session-1", 3, SessionStatus::Idle)],
                &running,
                false
            )
            .is_empty()
        );
    }

    #[test]
    fn stopping_a_running_session_clears_state_without_notifying() {
        use fxg_protocol::common::SessionStatus;

        let running = RunningTurns::default();
        notification_events(
            &[status_event("session-1", 1, SessionStatus::Running)],
            &running,
            false,
        );
        assert!(
            notification_events(
                &[status_event("session-1", 2, SessionStatus::Stopped)],
                &running,
                false
            )
            .is_empty()
        );
        // A later Idle must not be treated as the end of the stopped turn.
        assert!(
            notification_events(
                &[status_event("session-1", 3, SessionStatus::Idle)],
                &running,
                false
            )
            .is_empty()
        );
    }

    #[test]
    fn permissions_and_questions_are_notification_targets() {
        let events = [
            permission_event("session-1", 1),
            elicitation_event("session-1", 2),
        ];
        let selected = notification_events(&events, &RunningTurns::default(), false);
        assert_eq!(selected.len(), 2);
        assert!(
            notification_events(&events, &RunningTurns::default(), true).is_empty(),
            "resync must not wake devices"
        );
    }

    fn subscription(endpoint: &str, client: Option<&str>) -> PushSubscriptionRecord {
        PushSubscriptionRecord {
            endpoint: endpoint.to_owned(),
            p256dh: "p256dh".to_owned(),
            auth: "auth".to_owned(),
            device_name: None,
            client_id: client.map(ClientId::new),
        }
    }

    fn targeting(policy: TargetPolicy) -> NotificationTargeting {
        NotificationTargeting {
            policy,
            ..NotificationTargeting::default()
        }
    }

    fn record_presence(
        registry: &PresenceRegistry,
        client: &str,
        tab: &str,
        visible: bool,
        focused: bool,
        session: Option<&str>,
        at: Instant,
    ) {
        registry.record_at(
            crate::presence::ClientPresence {
                client_id: ClientId::new(client),
                tab_id: fxg_protocol::TabId::new(tab),
                visible,
                focused,
                focused_session_id: session.map(SessionId::new),
            },
            at,
        );
    }

    #[test]
    fn all_policy_targets_every_subscription() {
        let subscriptions = [
            subscription("legacy", None),
            subscription("c1", Some("client-1")),
        ];
        let targets = select_subscriptions(
            &subscriptions,
            &PresenceRegistry::default(),
            &SessionId::new("session-1"),
            &targeting(TargetPolicy::All),
            Instant::now(),
        );
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn active_only_targets_only_active_clients() {
        let registry = PresenceRegistry::default();
        let now = Instant::now();
        record_presence(&registry, "client-1", "tab-1", true, true, None, now);
        record_presence(&registry, "client-2", "tab-2", true, false, None, now);
        let subscriptions = [
            subscription("c1", Some("client-1")),
            subscription("c2", Some("client-2")),
            subscription("legacy", None),
        ];
        let targets = select_subscriptions(
            &subscriptions,
            &registry,
            &SessionId::new("session-1"),
            &targeting(TargetPolicy::ActiveOnly),
            now,
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].endpoint, "c1");
    }

    #[test]
    fn active_only_falls_back_to_all_when_no_client_is_active() {
        let subscriptions = [
            subscription("legacy", None),
            subscription("c1", Some("client-1")),
        ];
        let targets = select_subscriptions(
            &subscriptions,
            &PresenceRegistry::default(),
            &SessionId::new("session-1"),
            &targeting(TargetPolicy::ActiveOnly),
            Instant::now(),
        );
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn active_only_without_fallback_targets_nothing() {
        let subscriptions = [subscription("c1", Some("client-1"))];
        let selected = NotificationTargeting {
            fallback_all: false,
            ..NotificationTargeting::default()
        };
        let targets = select_subscriptions(
            &subscriptions,
            &PresenceRegistry::default(),
            &SessionId::new("session-1"),
            &selected,
            Instant::now(),
        );
        assert!(targets.is_empty());
    }

    #[test]
    fn active_or_recent_includes_a_client_that_was_blurred() {
        let registry = PresenceRegistry::default();
        let now = Instant::now();
        record_presence(&registry, "client-1", "tab-1", true, true, None, now);
        record_presence(&registry, "client-2", "tab-2", true, true, None, now);
        record_presence(
            &registry,
            "client-2",
            "tab-2",
            true,
            false,
            None,
            now + Duration::from_secs(10),
        );
        let subscriptions = [
            subscription("c1", Some("client-1")),
            subscription("c2", Some("client-2")),
        ];
        let later = now + Duration::from_secs(60);
        let targets = select_subscriptions(
            &subscriptions,
            &registry,
            &SessionId::new("session-1"),
            &targeting(TargetPolicy::ActiveOrRecent),
            later,
        );
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn most_recent_targets_only_the_latest_client() {
        let registry = PresenceRegistry::default();
        let now = Instant::now();
        record_presence(&registry, "client-1", "tab-1", true, true, None, now);
        record_presence(
            &registry,
            "client-2",
            "tab-2",
            true,
            true,
            None,
            now + Duration::from_secs(5),
        );
        let subscriptions = [
            subscription("c1", Some("client-1")),
            subscription("c2", Some("client-2")),
        ];
        let targets = select_subscriptions(
            &subscriptions,
            &registry,
            &SessionId::new("session-1"),
            &targeting(TargetPolicy::MostRecent),
            now + Duration::from_secs(10),
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].endpoint, "c2");
    }

    #[test]
    fn focused_session_is_suppressed_for_the_open_client() {
        let registry = PresenceRegistry::default();
        let now = Instant::now();
        record_presence(
            &registry,
            "client-1",
            "tab-1",
            true,
            true,
            Some("session-1"),
            now,
        );
        record_presence(
            &registry,
            "client-2",
            "tab-2",
            true,
            true,
            Some("session-2"),
            now,
        );
        let subscriptions = [
            subscription("c1", Some("client-1")),
            subscription("c2", Some("client-2")),
        ];
        let targets = select_subscriptions(
            &subscriptions,
            &registry,
            &SessionId::new("session-1"),
            &targeting(TargetPolicy::All),
            now,
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].endpoint, "c2");
    }
}
