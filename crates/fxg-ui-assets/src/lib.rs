//! Embeds the FlexAgent Web UI (`ui/build`) into the binary via `rust-embed`.
//!
//! - Embeds the SvelteKit + `@sveltejs/adapter-static` (SPA) build output so that
//!   `fxg server` / `fxg daemon` serve it from the same origin.
//! - When the UI has not been built, `build.rs` provides a placeholder `index.html`,
//!   so the Rust build itself always succeeds.

use rust_embed::RustEmbed;

/// Assets bundled from `ui/build`.
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../ui/build"]
pub struct UiAssets;

/// Fetches an asset (`path` is a relative path without a leading `/`).
pub fn get(path: &str) -> Option<rust_embed::EmbeddedFile> {
    UiAssets::get(path)
}

/// Resolution for SPA delivery: if the path is not found as-is and has no extension,
/// return `index.html`.
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
    // Client-side routes (e.g. `/sessions/<id>`) fall back to index.html
    let last_segment = normalized.rsplit('/').next().unwrap_or_default();
    if !last_segment.contains('.') {
        return get("index.html");
    }
    None
}

/// Infers the Content-Type from the extension (minimal mapping to avoid extra crates).
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

/// Cacheability: `index.html` and the Service Worker must reflect updates immediately.
pub fn cache_control(path: &str) -> &'static str {
    if path == "index.html" || path == "service-worker.js" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    }
}
