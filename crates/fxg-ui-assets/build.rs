//! `ui/build` が未生成でもコンパイルできるようにプレースホルダを用意する。
//!
//! リリースビルドでは先に `pnpm --dir ui build` (`mise run build:ui`) を実行し、
//! 生成物を `rust-embed` で同梱する。UI をビルドせずにコンパイルした場合は
//! 「UI がビルドされていません」と表示する最小の `index.html` が埋め込まれる。

use std::path::PathBuf;

const PLACEHOLDER_INDEX: &str = r#"<!doctype html>
<html lang="ja">
  <head>
    <meta charset="utf-8" />
    <title>FlexAgent</title>
  </head>
  <body style="font-family: system-ui; padding: 2rem">
    <h1>FlexAgent UI not built</h1>
    <p>
      このバイナリには Web UI が同梱されていません。<code>mise run build:ui</code>
      (または <code>pnpm --dir ui build</code>) を実行してから再度ビルドしてください。
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
        // 書込めない場合でも rust-embed 用にディレクトリだけは保証する
        println!("cargo:warning=failed to prepare ui/build placeholder: {err}");
        let _ = std::fs::create_dir_all(&ui_build);
    }
}
