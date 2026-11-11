# Hub / Node 接続 UX の修正とトークン用語の整理

- **日付**: 2026-11-11
- **対象パッケージ**: `fxg-cli`, `fxg-protocol`, `fxg-db`, `fxg-node`,
  `fxg-hub`, `ui`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run lint`,
  `mise run test`, `cargo check`, `cargo clippy`, `pnpm --dir ui run check`

## 背景

ハブの Web UI でノードを 0 件から追加するとき、次の問題があった。

- ノードが 0 件のとき、設定画面のノード選択が空になり、何を編集しているか
  分からない。
- ペアリングトークンを発行した直後に `node is not connected` トーストが
  大量に出る。ノード未接続時にも同じ症状が出る。
- 発行ダイアログの `echo ... > ~/.flexagent/node_token` がダイアログから
  はみ出す。
- `hub_url` に `ws://localhost:8080/api/v1/node/ws` を指定する必要があり、
  内部エンドポイントを知らないと設定できない。
- `auth_token`（Web UI / CLI 用）と `node_token`（hub ⇔ node 用）の
  区別が付きにくく、`fxg node token` というコマンド名も紛らわしかった。
- 到達不能時の画面が、ローカルノードを開いている場合でも
  `Cannot reach the hub` と表示していた。
- `~/.flexagent` にファイルが多く、どれが何なのか分からなかった。

## 原因

- ペアリングトークン発行時、未接続ノードを `is_online = true` の仮レコード
  として `nodes` に登録していた。UI はこれを「オンライン」と誤認し、
  `NODE_OFFLINE` になる問い合わせを繰り返していた。
- `AgentManager` のカタログ読み込み effect が `loading` を依存に持ち、
  失敗 → `loading` 変更 → effect 再実行のループになっていた。
- 新規セッション画面の `pickDefaultNode` が、オンラインノードが無い場合に
  オフラインノードへフォールバックしていた。
- 発行ダイアログの `<pre>` が `overflow-x-auto` でもダイアログ幅を超えて
  いた（実測: 本文 446px に対し 682px）。

## 変更

### UI

- すべてのノード選択に `No node online` を明示。設定画面の重複 `id` も解消。
- ノード未接続時は新規セッションのノード選択を無効化し、
  「オンラインのノードがない」と表示する。
- ペアリングトークン発行ダイアログを折り返し表示にし、
  `fxg node pair --hub <origin> --token <token>` をコピーできるようにした。
  手動設定は折りたたみへ移した。
- 到達不能画面を `Cannot reach the FlexAgent server` に変更し、
  接続先 URL と「hub または node を起動する / 接続先を切り替える」を表示。
- 用語を **Web access token** (`auth_token`) と
  **node pairing token** (`node_token`) に統一。

### CLI / 設定

- `fxg node pair --hub <URL> --token <TOKEN>` を追加。`node_token` を `0600`
  で保存し、`config.toml` の `[node] hub_url` をコメントを保ったまま更新する。
- `fxg node unpair` を追加。
- `hub_url` は `http://host:port` / `https://host:port` を受け付け、
  `ws(s)://.../api/v1/node/ws` を自動導出する。完全な WS URL も引き続き
  受け付ける。
- バナーと設定画面の Hub 表示を HTTP アドレスに変更。
- コマンド名を整理（破壊的変更）。
  - `fxg node token show|rotate` → `fxg node access-token show|rotate`
  - `fxg hub node-token issue|revoke|list` → `fxg hub pairing-token issue|revoke|list`
- `fxg doctor` に `~/.flexagent` のファイル一覧と用途を追加。

### ドキュメント

- `docs/05-cli-and-pwa-ui.md` §2.1 にファイル一覧と
  「どのトークンを使うか」の表を追加。
- `docs/01-architecture-and-sync.md` §7.3 のペアリング手順を
  `fxg node pair` に更新。
- `docs/03-protocol-and-api.md` の CLI 対応表に `fxg node pair` / `unpair`
  を追加。

## ファイル名について

`auth_token` / `node_token` は既存データとの互換性とドキュメントの参照が
多いため変更しない。代わりに UI・CLI・docs で用途を明示する。

## 検証

- ハブを一時 `FXG_HOME` で起動し、ノード 0 件の設定画面で
  `No node online` が出ること、トークン発行後にトーストが増えないことを確認。
- `fxg node pair --hub http://127.0.0.1:8080 --token ...` で
  `node_token` (`0600`) と `[node] hub_url` が書かれ、既存の
  `config.toml` のコメントが残ることを確認。
- `cargo test --workspace`, `cargo clippy --workspace --all-targets`,
  `pnpm --dir ui run check`, `pnpm --dir ui run test` を通す。
