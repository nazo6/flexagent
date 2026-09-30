//! VAPID Web Push 送信 (中央サーバー)。
//!
//! 設計: `docs/03-protocol-and-api.md` §3.1、`docs/05-cli-and-pwa-ui.md` §3.3。
//!
//! - VAPID 鍵ペアは初回起動時に生成し `~/.flexagent/vapid.json` へ保存する
//!   (公開鍵は `GET /api/v1/system/info` の `vapid_public_key` で配布)。
//! - 承認リクエスト (`PermissionRequest` イベント) の適用時に、登録済みの
//!   全購読へ通知を fan-out する。Android の通知バナー上の Approve / Reject は
//!   Service Worker が `POST .../permissions/:req_id/respond` を叩く。
//! - 暗号化 (RFC 8291 aes128gcm) と VAPID 署名 (RFC 8292) は純 Rust の
//!   [`web_push_native`] (`p256` / `aes-gcm` / `hkdf`) で行い、OpenSSL に依存
//!   しない。HTTP 送信はワークスペース共通の `reqwest` を使用する。

use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use fxg_db::{Db, PushSubscriptionRecord};
use serde::{Deserialize, Serialize};
use web_push_native::jwt_simple::algorithms::ES256KeyPair;
use web_push_native::p256::elliptic_curve::sec1::ToEncodedPoint;
use web_push_native::{Auth, WebPushBuilder};

/// VAPID 鍵ファイル名 (`~/.flexagent/vapid.json`)。
pub const VAPID_KEYS_FILE: &str = "vapid.json";

/// VAPID の `sub` claim (連絡先)。
const VAPID_CONTACT: &str = "mailto:flexagent@localhost";

/// VAPID 鍵ペア (base64url / no padding)。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VapidKeys {
    /// P-256 秘密鍵 (32 バイトスカラの base64url)
    private_key: String,
    /// P-256 公開鍵 (非圧縮点 65 バイトの base64url)
    public_key: String,
}

/// Web Push 送信エラー。
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// VAPID 鍵ファイルの入出力エラー
    #[error("vapid key io error: {0}")]
    Io(#[from] std::io::Error),
    /// VAPID 鍵 / ペイロードの JSON エラー
    #[error("push json error: {0}")]
    Json(#[from] serde_json::Error),
    /// P-256 鍵生成に失敗 (乱数が有効なスカラにならなかった)
    #[error("failed to generate a vapid keypair")]
    KeyGeneration,
    /// Push エンドポイント URL が不正
    #[error("invalid push endpoint: {0}")]
    InvalidEndpoint(String),
    /// 購読の鍵 (`p256dh` / `auth`) が不正
    #[error("invalid push subscription keys")]
    InvalidKeys,
    /// 署名・暗号化エラー (web-push-native)
    #[error("web push build error: {0}")]
    Build(#[from] web_push_native::Error),
    /// HTTP 送信エラー
    #[error("push request error: {0}")]
    Http(#[from] reqwest::Error),
    /// Push サービスがエラーステータスを返した
    #[error("push endpoint returned status {0}")]
    HttpStatus(u16),
}

/// 通知ペイロード (Service Worker の `push` ハンドラが解釈する)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushNotificationPayload {
    /// 通知タイトル (例: `[flexagent] 承認リクエスト (Home-Win)`)
    pub title: String,
    /// 通知本文 (例: `opencode2: Run "cargo test --workspace"`)
    pub body: String,
    /// 対象セッションID
    pub session_id: String,
    /// 承認リクエストID
    pub request_id: String,
    /// 通知の置換タグ (同じリクエストの通知を上書きする)
    pub tag: String,
    /// `[Approve]` ボタンが送る `option_id`
    pub allow_option_id: Option<String>,
    /// `[Reject]` ボタンが送る `option_id`
    pub reject_option_id: Option<String>,
}

/// 送信結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushSendOutcome {
    /// 送信成功
    Sent,
    /// 購読が失効 (`404` / `410`。購読を削除する)
    Gone,
}

/// VAPID 鍵を保持し Web Push を送信するサービス。
#[derive(Clone)]
pub struct PushService {
    client: reqwest::Client,
    key_pair: Arc<ES256KeyPair>,
    public_key: String,
}

impl PushService {
    /// `~/.flexagent/vapid.json` を読み込む (無ければ生成して保存する)。
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
        let key_pair =
            ES256KeyPair::from_bytes(&private_bytes).map_err(|_| PushError::InvalidKeys)?;
        Ok(Self {
            client: reqwest::Client::new(),
            key_pair: Arc::new(key_pair),
            public_key: keys.public_key,
        })
    }

    /// `GET /api/v1/system/info` で配布する VAPID 公開鍵 (base64url)。
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// 1 購読向けの HTTP プッシュリクエスト (暗号化 + VAPID 署名済み) を構築する。
    fn build_request(
        &self,
        subscription: &PushSubscriptionRecord,
        body: Vec<u8>,
    ) -> Result<http::Request<Vec<u8>>, PushError> {
        let endpoint: http::Uri = subscription
            .endpoint
            .parse()
            .map_err(|_| PushError::InvalidEndpoint(subscription.endpoint.clone()))?;
        let ua_public_bytes = decode_base64(&subscription.p256dh).ok_or(PushError::InvalidKeys)?;
        let ua_public = web_push_native::p256::PublicKey::from_sec1_bytes(&ua_public_bytes)
            .map_err(|_| PushError::InvalidKeys)?;
        let auth_bytes = decode_base64(&subscription.auth).ok_or(PushError::InvalidKeys)?;
        if auth_bytes.len() != 16 {
            return Err(PushError::InvalidKeys);
        }
        let auth = Auth::clone_from_slice(&auth_bytes);
        let builder = WebPushBuilder::new(endpoint, ua_public, auth)
            .with_vapid(&self.key_pair, VAPID_CONTACT);
        Ok(builder.build(body)?)
    }

    /// 1 購読へ通知を送る。
    pub async fn send(
        &self,
        subscription: &PushSubscriptionRecord,
        payload: &PushNotificationPayload,
    ) -> Result<PushSendOutcome, PushError> {
        let body = serde_json::to_vec(payload)?;
        let request = self.build_request(subscription, body)?;

        // `http::Request` を reqwest で送る
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

/// P-256 鍵ペアを生成する (公開鍵は非圧縮 SEC1 形式で保存する)。
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

/// base64url (no padding) または標準 base64 をデコードする。
fn decode_base64(value: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| STANDARD.decode(value))
        .ok()
}

/// `PermissionRequest` イベントから通知ペイロードを組み立てる。
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
        Some(title) => format!("[flexagent] 承認リクエスト ({node_label}) — {title}"),
        None => format!("[flexagent] 承認リクエスト ({node_label})"),
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

/// イベント列内の `PermissionRequest` を全購読へ通知する。
///
/// 失効した購読 (`404` / `410`) は削除する。失敗は警告ログのみで継続する。
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

        for subscription in &subscriptions {
            match push.send(subscription, &payload).await {
                Ok(PushSendOutcome::Sent) => {
                    tracing::debug!(endpoint = %subscription.endpoint, request_id, "push sent");
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

    /// テスト用の UA (ブラウザ) 側購読を生成する。
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
            title: "[flexagent] 承認リクエスト (Home-Win)".to_owned(),
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

        // VAPID 署名ヘッダが付与される
        let authorization = request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .expect("authorization header");
        assert!(authorization.starts_with("vapid t="), "{authorization}");
        assert!(authorization.contains("k="), "{authorization}");
        assert!(request.headers().contains_key("content-encoding"));

        // サーバーが唯一の受信者である UA 側で復号できる (RFC 8291 の往復検証)
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
            "[flexagent] 承認リクエスト (Home-Win) — Fix auth"
        );
        assert_eq!(payload.body, "terminal/create: Run cargo test");
        assert_eq!(payload.tag, "fxg-permission-req-1");
        assert_eq!(payload.allow_option_id.as_deref(), Some("allow_once"));
        assert_eq!(payload.reject_option_id.as_deref(), Some("reject"));
    }
}
