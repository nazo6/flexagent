# Fork capability の契約 (`fork_scope` / `up_to_node_seq`)

- **日付**: 2026-11-12
- **対象パッケージ**: `fxg-protocol` / `fxg-db` / `fxg-acp` / `fxg-node` /
  `fxg-hub` / `fxg-cli` / `ui`
- **対象スクリプト**: `cargo test -p fxg-protocol -p fxg-db -p fxg-acp -p fxg-node -p fxg-hub`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `pnpm --dir ui check`, `pnpm --dir ui test`, `pnpm --dir ui lint`,
  `mise run sqlx:prepare`, `mise run types`, `mise run fmt`

## 背景

Fork は ACP ネイティブの `session/fork` + `session/load` のみを提供しており、
対応可否は `fork_supported: bool` で表していた。メッセージ単位の Fork を
UI に追加するには、「どこまで Fork できるか」を capability として表現し、
Fork 元のイベント境界を API で指定できる必要がある。

## 変更

- `ForkScope` (`none` / `last` / `message`) を追加し、`SessionSummary` と
  `CapabilitiesUpdated` の `fork_supported: bool` を `fork_scope` に置き換えた。
  `sessions.fork_supported` は migration `0021_sessions_fork_scope.sql` で
  `sessions.fork_scope TEXT` へ移行する（`1` → `last`、`0` → `none`）。
- `SessionForkSpec` に `up_to_node_seq: Option<u64>` を追加し、Node⇔Hub の
  `StartSession` にも `fork_up_to_node_seq` を追加した。
- ACP は `sessionCapabilities.fork` と `loadSession` の両方がある場合に
  `ForkScope::Last` を広告する。`message` はまだ広告しない。
- node は `up_to_node_seq` を Fork 元の `last_node_seq` と照合する。
  末尾より前の境界は `FORK_SCOPE_UNSUPPORTED` で拒否し、Fork 先のセッションを
  開始しない。末尾への Fork は従来どおり会話全体を複製する。
- UI は生成された `ForkScope` 型を使い、`fork_scope` から Fork 可否を判定する。
  `FORK_SCOPE_UNSUPPORTED` のエラーメッセージを追加した。

## 不変条件

- `fork_scope = none` のセッションは Fork できない。
- `fork_scope = last` は現在の会話末尾からのみ Fork でき、途中の
  `up_to_node_seq` は拒否される。
- `fork_scope = message` は将来の ACP 拡張用に予約する。現行の ACP ネイティブ
  Fork は `message` を広告しない。
- Fork 先には `parent_session_id` と、Fork 時点までの会話イベントだけを複製する。
- DB の未知の `fork_scope` は既定値に丸めず decode エラーにする。

## 検証

- `fork_scope_projection_tracks_latest_and_rebuilds`（DB 投影と再構築）
- `fork_rejects_a_boundary_before_the_latest_event`（node の境界検証）
- 既存の `fork_uses_native_session_fork` / `start_session_with_fork_from_uses_native_fork`
- UI の型チェックとテスト
