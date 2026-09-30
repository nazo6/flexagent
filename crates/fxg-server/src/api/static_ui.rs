//! `rust-embed` で同梱した Web UI (`ui/build`) の配信。
//!
//! `fxg server` / `fxg daemon` の共通ルーターのフォールバックとして、
//! API 以外の全パスを SPA として配信する (設計: `docs/05-cli-and-pwa-ui.md`)。
//!
//! - 認証ミドルウェアの外側にあるため、UI アセット自体はトークン不要で
//!   配信される (API は 401 を返し、UI がトークン入力ダイアログを表示する)。
//! - `index.html` / `service-worker.js` は `no-cache`、それ以外のビルド
//!   アセットはハッシュ付きファイル名のため長期キャッシュとする。

use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

/// SPA 配信ハンドラ (API ルートに一致しなかった全リクエストのフォールバック)。
pub async fn serve_ui(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let resolved = if path.is_empty() { "index.html" } else { path };

    let Some(file) = fxg_ui_assets::resolve(resolved) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    // resolve が index.html へフォールバックした場合はキャッシュ制御も合わせる
    let is_fallback = resolved != "index.html" && fxg_ui_assets::get(resolved).is_none();
    let effective = if is_fallback { "index.html" } else { resolved };

    let body = Body::from(file.data.into_owned());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, fxg_ui_assets::content_type(effective))
        .header(
            header::CACHE_CONTROL,
            fxg_ui_assets::cache_control(effective),
        )
        .body(body)
        .unwrap_or_else(|err| {
            tracing::warn!("failed to build ui response: {err}");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        })
}
