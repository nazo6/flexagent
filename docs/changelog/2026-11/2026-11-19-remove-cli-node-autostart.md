# CLI からのノード自動起動を廃止

- **日付**: 2026-11-19
- **対象パッケージ**: `fxg-cli`, `fxg-protocol`
- **対象スクリプト / 設定**: `mise run fmt`, `cargo check -p fxg-cli`,
  `cargo test -p fxg-cli`, `cargo test -p fxg-protocol`

## 背景

`NodeClient::connect()` は未起動のノードを `fxg node start` の detached
起動で補っていた。この挙動のために `<state>/node/start.lock`、stale ロック
判定、15 秒の readiness ポーリング、`FXG_NO_AUTO_NODE` という逃げ道が必要で、
`fxg session list` のような読み取りコマンドが常駐プロセスを増やす副作用も
あった。

## 変更

- `NodeClient::connect()` から自動起動を削除し、`connect_existing()` と統合した。
  未起動時は `fxg node start --detach` を案内するエラーを返す。
- `FXG_NO_AUTO_NODE` と `env_keys::NO_AUTO_NODE` を削除した。
- `<state>/node/start.lock` と `Paths::start_lock_path()` を削除した。
- `fxg`（引数なし）/ `fxg web` も未起動ならエラーになる。ノードの起動は
  `fxg node start` または `fxg service install node` に一本化した。
- `cli_e2e` の自動起動向け env とテスト名を更新し、`docs/03` / `docs/05` の
  記述を合わせた。

## 理由

- 読み取りコマンドが暗黙にバックグラウンドプロセスを起動するのは驚きが大きい。
- OS サービス登録と `fxg node start` の service 委譲があるため、常駐は明示的に
  管理できる。自動起動の実装コストが 1 コマンドの節約に見合わない。
- `FXG_NO_AUTO_NODE` が「暗黙起動をやめさせる」ためだけに存在しており、
  設定の所有者という方針からも外れていた。

## 影響

- 新規環境では `fxg node start --detach`（または `fxg service install node`）が
  最初に必要になる。
- スクリプト / CI はノード未起動を早期に検出でき、意図しない常駐プロセスが
  増えない。

## 検証

- `cargo check -p fxg-cli`
- `cargo test -p fxg-protocol paths`
- `cargo test -p fxg-cli --test cli_e2e`
