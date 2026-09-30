# 自動生成 TypeScript 型定義 (生成物はコミットする)

このディレクトリのファイルは `fxg-protocol` の `#[ts(export)]` から `ts-rs`
によって自動生成される。手で編集しないこと。

- 生成: `cargo test -p fxg-protocol` (または `mise run types`)
- 出力先設定: リポジトリルートの `.cargo/config.toml` (`TS_RS_EXPORT_DIR`)
- 生成元: `crates/fxg-protocol/src/**` (`#[ts(export)]` が付与された型)

Rust 側の型を変更した場合は必ず再生成し、生成差分もコミットすること
(`docs/README.md` / `AGENTS.md` の開発ルール参照)。
