//! Embeds the FlexAgent Web UI (`ui/build`) into the binary via `rust-embed`.
//!
//! - Embeds the SvelteKit + `@sveltejs/adapter-static` (SPA) build output so that
//!   `fxg hub start` / `fxg node start` serve it from the same origin.
//! - When the UI has not been built, `build.rs` provides a placeholder `index.html`,
//!   so the Rust build itself always succeeds.

#![allow(
    clippy::disallowed_methods,
    reason = "the rust-embed derive expands Result::ok internally"
)]

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

/// Infers the Content-Type from the extension.
///
/// `mime_guess` owns the extension table; text formats get an explicit UTF-8
/// charset and source maps keep the JSON type the browser devtools expect.
pub fn content_type(path: &str) -> String {
    if path.ends_with(".map") {
        return "application/json; charset=utf-8".to_owned();
    }
    let mime = mime_guess::from_path(path)
        .first_raw()
        .unwrap_or("application/octet-stream");
    if mime.starts_with("text/")
        || matches!(
            mime,
            "application/json" | "application/javascript" | "application/manifest+json"
        )
    {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_owned()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_covers_build_assets() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("app.js"), "text/javascript; charset=utf-8");
        assert_eq!(
            content_type("app.mjs"),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(content_type("app.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("data.json"), "application/json; charset=utf-8");
        assert_eq!(
            content_type("bundle.js.map"),
            "application/json; charset=utf-8"
        );
        assert_eq!(
            content_type("app.webmanifest"),
            "application/manifest+json; charset=utf-8"
        );
        assert_eq!(content_type("icon.svg"), "image/svg+xml");
        assert_eq!(content_type("icon.png"), "image/png");
        assert_eq!(content_type("font.woff2"), "font/woff2");
        assert_eq!(content_type("module.wasm"), "application/wasm");
        assert_eq!(content_type("photo.avif"), "image/avif");
        assert_eq!(content_type("font.ttf"), "font/ttf");
        assert_eq!(content_type("unknown.zzz"), "application/octet-stream");
    }
}
