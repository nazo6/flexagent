# 自動生成 TypeScript 型定義 (生成物はコミットする)

`bindings.ts` は `fxg-protocol` の型から
[specta](https://github.com/specta-rs/specta) によって自動生成される。
手で編集しないこと。

- 生成: `cargo test -p fxg-protocol export_bindings` (または `mise run types`)
- 生成元: `crates/fxg-protocol/src/export_bindings.rs`
  (`#[specta(collect)]` を付与した型と、それらが参照する型)
- import: `import type { SessionSummary } from "#lib/generated/bindings";`
- 出力は単一ファイル (`Layout::FlatFile`)。型名の昇順で安定して出力される。

Rust 側の型を変更した場合は必ず再生成し、生成差分もコミットすること
(`docs/README.md` / `AGENTS.md` の開発ルール参照)。
