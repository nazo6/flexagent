//! Provides a placeholder so that compilation works even when `ui/build` is absent.
//!
//! Release builds run `pnpm --dir ui build` (`mise run build:ui`) first and embed the
//! output via `rust-embed`. When compiling without building the UI, a minimal
//! `index.html` reporting that the UI has not been built is embedded instead.

use std::path::PathBuf;

const PLACEHOLDER_INDEX: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>FlexAgent</title>
  </head>
  <body style="font-family: system-ui; padding: 2rem">
    <h1>FlexAgent UI not built</h1>
    <p>
      This binary does not bundle the Web UI. Run <code>mise run build:ui</code>
      (or <code>pnpm --dir ui build</code>) and rebuild.
    </p>
  </body>
</html>
"#;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let ui_build = manifest_dir.join("../../ui/build");
    println!("cargo:rerun-if-changed={}", ui_build.display());

    let placeholder = ui_build.join("index.html");
    if !placeholder.exists()
        && let Err(err) = std::fs::create_dir_all(&ui_build)
            .and_then(|()| std::fs::write(&placeholder, PLACEHOLDER_INDEX))
    {
        // Guarantee the directory exists for rust-embed even if the write fails
        println!("cargo:warning=failed to prepare ui/build placeholder: {err}");
        if let Err(retry_err) = std::fs::create_dir_all(&ui_build) {
            println!("cargo:warning=failed to create ui/build: {retry_err}");
        }
    }
}
