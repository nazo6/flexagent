# エージェント設定の環境変数整理

- **日付**: 2026-11-19
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-agent`, `fxg-node`,
  `ui`（生成型）
- **対象スクリプト / 設定**: `mise run sqlx:prepare`, `mise run types`,
  `mise run fmt`, `cargo clippy`, `cargo test`, `pnpm --dir ui run check`

## 背景

`docs/05` §2 は「実行時に変更する設定は所有者の DB に保存する」と定めているが、
次の 2 つは起動後にしか使わない値なのに環境変数にあった。

- `FXG_AGENT_WARM_IDLE_SECS`: ウォーム ACP プロセスの idle TTL。
  `agent_settings` の `registry_cache_ttl_secs` と同じ種類の値なのに、保存先が
  分かれていた。settings API にも UI にも現れない。
- `FXG_OPENCODE_URL` / `FXG_OPENCODE_PASSWORD`: テスト用に追加された接続先
  オーバーライドが本番のドライバ選択に残り、ローカルの service discovery より
  優先されていた。子プロセスに継承されるため、テスト用の export が通常の
  セッションを外部サーバーへ繋ぎ得る状態だった。

## 変更

### ウォームプール TTL を `node.db` へ

- `0004_agent_warm_idle.sql` で `agent_settings.warm_idle_secs` を追加した。
  NULL は既定（600 秒）、`0` はノード終了まで保持を表す。
- `AgentSettings` / `UpdateAgentSettingsRequest` に `warm_idle_secs` と
  `resolved_warm_idle_secs()` を追加した。
- `WarmPool::ttl` を `Mutex<Duration>` にし、`set_ttl` で実行時に差し替えられる
  ようにした。`AcpDriver::apply_catalog` 経由で設定変更が即時反映される。
- `0` のときは期限タイマーを張らない。従来は `Duration::ZERO` で即時破棄されて
  おり、「ノード終了まで保持」という説明と一致していなかった。
- `FXG_AGENT_WARM_IDLE_SECS` と `env_keys::AGENT_WARM_IDLE_SECS` を削除した。

### OpenCode の接続先オーバーライドを削除

- `env_client` / `env_var` を削除し、`OpenCodeNativeDriver` は
  injected → discovered → spawn の順だけにした。
- 実サーバー向けテストは service discovery を使い、明示注入は既存の
  `with_server` を使う。

### 環境変数の許可範囲を明文化

- `docs/05` §2.4 に、`FXG_*` を追加してよいのは「起動前 bootstrap」
  「エコシステム標準（`RUST_LOG` / `NO_COLOR` / `CLICOLOR_FORCE`）」
  「`cfg(test)` に隔離されたフック」の 3 種類だけであると追記した。
- `docs/04` §2.4 に、ウォームプールの保持時間は `agent_settings.warm_idle_secs`
  が所有し、再起動なしで反映されることを追記した。

## 影響

- Client API から `warm_idle_secs` を設定できる（省略時は既定 600 秒）。
- `FXG_AGENT_WARM_IDLE_SECS` / `FXG_OPENCODE_URL` / `FXG_OPENCODE_PASSWORD` は
  読まれなくなる。
- `warm_idle_secs = 0` が意図どおり「ノード終了まで保持」として機能する。

## 検証

- `mise run sqlx:check`
- `cargo clippy -p fxg-protocol -p fxg-db -p fxg-agent -p fxg-node --all-targets`
- `cargo test -p fxg-db --test settings`
- `cargo test -p fxg-agent --lib acp::warm`
- `pnpm --dir ui run check`
