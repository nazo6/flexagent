//! FlexAgent Web UI (`ui/build`) を `rust-embed` でバイナリへ同梱する。
//!
//! - SvelteKit + `@sveltejs/adapter-static` (SPA) のビルド成果物を埋め込み、
//!   `fxg server` / `fxg daemon` が同一オリジンで配信する。
//! - UI 未ビルド時は `build.rs` がプレースホルダ `index.html` を用意するため、
//!   Rust のビルド自体は常に成功する。

use rust_embed::RustEmbed;

/// `ui/build` の同梱アセット。
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../ui/build"]
pub struct UiAssets;

/// アセットを取得する (`path` は先頭 `/` なしの相対パス)。
pub fn get(path: &str) -> Option<rust_embed::EmbeddedFile> {
    UiAssets::get(path)
}

/// SPA 配信向けの解決: そのまま見つからず拡張子なしのパスなら `index.html` を返す。
pub fn resolve(path: &str) -> Option<rust_embed::EmbeddedFile> {
    let normalized = path.trim_start_matches('/');
    let normalized = if normalized.is_empty() {
        "index.html"
    } else {
        normalized
    };
    if let Some(file) = get(normalized) {
        return Some(file);
    }
    // クライアントサイドルーティング (`/sessions/<id>` 等) は index.html へ
    let last_segment = normalized.rsplit('/').next().unwrap_or_default();
    if !last_segment.contains('.') {
        return get("index.html");
    }
    None
}

/// 拡張子から Content-Type を推定する (外部クレートを増やさない最小の対応表)。
pub fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "webmanifest" => "application/manifest+json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// キャッシュ可否: `index.html` と Service Worker は更新を即時反映させる。
pub fn cache_control(path: &str) -> &'static str {
    if path == "index.html" || path == "service-worker.js" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    }
}
