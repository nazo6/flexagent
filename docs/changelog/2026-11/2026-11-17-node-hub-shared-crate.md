# `fxg-node` → `fxg-hub` 依存の共有クレート分離

- **日付**: 2026-11-17
- **対象パッケージ**: `fxg-api`（新規）, `fxg-hub`, `fxg-node`, `fxg-cli`, `docs`
- **対象スクリプト**: `cargo test --workspace --no-fail-fast`,
  `cargo clippy --workspace --all-targets`, `mise run lint:errors`,
  `mise run fmt:check`

---

## 概要

`fxg-node` が本番コードで `fxg-hub` に依存していた。実際に使っていたのは
Client API の共有ルーター（`fxg_hub::api`）、Web Push（`fxg_hub::push`）、
シャットダウン補助の3系統だけだったが、依存の向きが「ノード → ハブ」になって
おり、ノード単体ではビルドできず、ハブ固有の `hub.rs` / `state` /
`provisioner` まで常に引きずっていた。

共有境界はもともと `ClientApiBackend` トレイトとして存在していたため、新しい
抽象を導入せず、その境界ごと新クレート `fxg-api` へ移した。移動のみで、
公開 API・wire format・挙動は変えていない。

## 変更

### 新規: `fxg-api`

- `crates/fxg-hub/src/api/` を `crates/fxg-api/src/` へ移動（`mod.rs` は
  `lib.rs` 化）。
- `crates/fxg-hub/src/push.rs` を `fxg_api::push` へ移動。
- `crates/fxg-hub/src/process.rs` の `spawn_ctrl_c_handler` /
  `await_shutdown_task` を `fxg_api::shutdown` へ移動。
- `header_str` と `send_ws_json` はハブ側から使うため `pub` にした。

### 依存関係

- `fxg-node` の本番依存を `fxg-hub` から `fxg-api` へ変更。
- `fxg-node` の `fxg-hub` は `sync_e2e` テストがハブを起動するため
  `[dev-dependencies]` に移動。
- `fxg-hub` は `fxg-api` に依存し、`hub.rs` / `state` / `provisioner` /
  `process::Hub` だけを保持する。
- `fxg-hub` から不要になった `fs4` / `rand` / `reqwest` / `sha2` /
  `web-push-native` / `fxg-ui-assets` を削除。
- `fxg-cli` は `fxg-api` を直接依存する（`resolve_log_file` / `auth`）。

### ドキュメント

- `docs/README.md` のクレート構成に `fxg-api` を追加。
- `docs/03-protocol-and-api.md` の Client API 実装先を `crates/fxg-api/src/`
  に更新。
- `docs/06-type-safety.md` の対象クレートに `fxg-api` を追加。

## 検証

- `cargo test --workspace --no-fail-fast`
- `cargo clippy --workspace --all-targets`
- `mise run lint:errors`
- `mise run fmt:check`
- `cargo tree -p fxg-node --edges normal` に `fxg-hub` が出ないこと
- `cargo tree -i fxg-hub --edges normal` が `fxg-cli` だけであること
