//! Serving the Web UI (`ui/build`) embedded with `rust-embed`.
//!
//! As the fallback of the shared router used by `fxg server` / `fxg daemon`, every non-API
//! path is served as the SPA (design: `docs/05-cli-and-pwa-ui.md`).
//!
//! - Because this sits outside the authentication middleware, the UI assets themselves are
//!   served without a token (the API returns 401 and the UI prompts for a token).
//! - `index.html` / `service-worker.js` are `no-cache`; all other build assets have hashed
//!   filenames and are cached long-term.

use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

/// SPA serving handler (fallback for every request that matched no API route).
pub async fn serve_ui(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let resolved = if path.is_empty() { "index.html" } else { path };

    let Some(file) = fxg_ui_assets::resolve(resolved) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    // If resolve fell back to index.html, the cache policy must follow it too
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
