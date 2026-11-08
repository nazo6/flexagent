//! VAPID Web Push delivery (central server).
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

use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use fxg_db::{Db, PushSubscriptionRecord};
use serde::{Deserialize, Serialize};
use web_push_native::jwt_simple::algorithms::ES256KeyPair;
use web_push_native::p256::elliptic_curve::sec1::ToEncodedPoint;
use web_push_native::{Auth, WebPushBuilder};

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
    /// Notification body (e.g. `opencode2: Run "cargo test --workspace"`)
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
                tracing::info!(path = %path.display(), "generated vapid keypair");
                keys
            }
            Err(err) => return Err(err.into()),
        };
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
        })
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
        let builder = WebPushBuilder::new(endpoint, ua_public, auth)
            .with_vapid(&self.key_pair, VAPID_CONTACT);
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
        .find(|option| option.kind.starts_with("allow"))
        .map(|option| option.option_id.clone());
    let reject = options
        .iter()
        .find(|option| option.kind.starts_with("reject"))
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

/// Sends one payload to every subscription (expired subscriptions are deleted). Failures only
/// produce a warning log.
async fn send_to_subscriptions(
    push: &PushService,
    db: &Db,
    subscriptions: &[PushSubscriptionRecord],
    payload: &PushNotificationPayload,
) {
    for subscription in subscriptions {
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
                tracing::warn!(endpoint = %subscription.endpoint, "push send failed: {err}");
            }
        }
    }
}

/// Notifies every subscription about `PermissionRequest`s in an event batch.
///
/// Expired subscriptions (`404` / `410`) are deleted. Failures only produce a warning log and
/// processing continues.
pub async fn notify_permission_requests(
    push: &PushService,
    db: &Db,
    events: &[fxg_protocol::events::SessionEventEnvelope],
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
                event.session_id.clone(),
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
            .map(|session| session.node_id.clone())
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

        send_to_subscriptions(push, db, &subscriptions, &payload).await;
    }
}

/// Notifies every subscription about `ElicitationRequest`s in an event batch.
///
/// Expired subscriptions (`404` / `410`) are deleted. Failures only produce a warning log and
/// processing continues.
pub async fn notify_elicitations(
    push: &PushService,
    db: &Db,
    events: &[fxg_protocol::events::SessionEventEnvelope],
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
                event.session_id.clone(),
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
            .map(|session| session.node_id.clone())
            .unwrap_or_else(|| "unknown".to_owned());
        let payload = elicitation_notification(
            &session_id,
            session.as_ref().map(|session| session.title.as_str()),
            &node_label,
            &elicitation_id,
            &message,
        );

        send_to_subscriptions(push, db, &subscriptions, &payload).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::common::PermissionOption;

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
        };
        (record, secret, auth)
    }

    #[test]
    fn vapid_keys_are_persisted_and_reloaded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = PushService::load_or_create(dir.path()).expect("generate");
        assert!(!first.public_key().is_empty());
        assert!(dir.path().join(VAPID_KEYS_FILE).exists());
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
                kind: "reject_once".to_owned(),
            },
            PermissionOption {
                option_id: "allow_once".to_owned(),
                name: "Allow".to_owned(),
                kind: "allow_once".to_owned(),
            },
            PermissionOption {
                option_id: "allow_always".to_owned(),
                name: "Always".to_owned(),
                kind: "allow_always".to_owned(),
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
}
