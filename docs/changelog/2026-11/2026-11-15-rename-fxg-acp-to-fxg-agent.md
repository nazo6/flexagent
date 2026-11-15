# `fxg-acp` を `fxg-agent` へリネームし、ドライバ層の構成を整理

- **日付**: 2026-11-15
- **対象パッケージ**: `fxg-agent`（旧 `fxg-acp`）, `fxg-node`, `fxg-cli`,
  `docs`
- **対象スクリプト**: `mise run fmt`, `cargo check --workspace --all-targets`,
  `cargo clippy --workspace --all-targets`, `cargo test --workspace`

`fxg-acp` は `AcpDriver` だけでなく Tier 2 の `ExtendedAcpDriver` と Tier 3 の
`OpenCodeNativeDriver`、`AgentCatalog`、ACP Registry、認証を内包しており、
クレート名が実態（エージェントドライバ層）と一致していなかった。クレートを
`fxg-agent` にリネームし、`AgentDriver` を中心とした名前へ揃えた。

## 変更

- `crates/fxg-acp/` を `crates/fxg-agent/` に移動した。パッケージ名・
  workspace 依存・`fxg-node` / `fxg-cli` の依存を `fxg-agent` に更新した。
- `src/agent/` モジュールをフラット化した。
  - `agent/catalog.rs` → `src/catalog.rs`
  - `agent/provider.rs` → `src/provider.rs`
  - `agent/builtin.rs` → `src/builtin.rs`
  - `agent/extension.rs` → `src/extension.rs`
  - `agent/extended.rs` → `src/extended.rs`
  - `agent/opencode.rs` → `src/opencode.rs`
  - 再エクスポートは `lib.rs` に集約し、`fxg_agent::agent::...` の重複パスを
    解消した。
- `docs/README.md`, `docs/03`, `docs/04`, `docs/06`, `AGENTS.md` のパス・
  クレート名を更新した。`docs/04` のタイトルから「ACP一本化」を外し、
  ACP + ネイティブ API の実態に合わせた。
- `fxg-mock-acp` バイナリ名、`DriverKind` / `AgentBackendKind` の値、
  プロトコル型、DB スキーマは変更していない。specta の型再生成は不要。

## 検証

- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets`
- `cargo test --workspace`
- `git grep "fxg-acp"` の残存が過去の changelog / proposal のみであること
