# リモート PTY 設定 `allow_remote_pty` の削除

- **日付**: 2026-11-26
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-api`, `fxg-node`, `ui`, `docs`
- **対象スクリプト**: `mise run sqlx:prepare` / `mise run types` /
  `cargo test -p fxg-protocol -p fxg-db` / `pnpm --dir ui run check`

## 背景

`node_settings.allow_remote_pty` はハブ経由の Web PTY 起動を拒否する設定だったが、
セキュリティ境界として成立していなかった。

- ハブの Client API はノード設定の更新をそのまま対象ノードへプロキシするため、
  ハブに到達できるクライアントは `allow_remote_pty` を自分で有効化してから
  PTY を開けた。
- ノードへ直接接続したクライアントには元々適用されなかった。
- ハブ / ノードのトークンを持つクライアントは、エージェント経由でも任意の
  コマンドを実行できる。

そのため、UI に「セキュリティ設定」として見えるものの実効性がない状態だった。

## 変更

- `NodeSettings` / `UpdateNodeSettingsRequest` から `allow_remote_pty` を削除した。
- ノードの `pty_open` からハブ経由判定による拒否を削除した。`PtyOpenParams.remote`
  は監査ソースの記録にのみ使う。
- `ErrorCode::PtyDisabled` と `PTY_DISABLED` の文字列、UI の専用メッセージ・
  「blocked」表示を削除した。
- ノード設定 UI から「Allow remote Web PTY」を削除した。
- マイグレーション `0005_remove_allow_remote_pty.sql` で
  `node_settings.allow_remote_pty` 列を削除した。
- ノード ⇔ ハブ間の設定メッセージ形が変わるため `PROTOCOL_VERSION` を 2 に上げた。

## 不変条件

1. Web PTY の可否はトークン認証とネットワーク境界で決まる。経路（ハブ / ノード直結）
   によって差を設けない。
2. PTY の cwd は登録済みプロジェクトパスまたは既存セッションの `local_path` に
   完全一致するものだけを許可する（この制限は維持）。
3. `PtyOpenParams.remote` は監査上の送信元区別だけに使う。
