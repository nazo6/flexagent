# Terminal ツールのマルチタブ化

- **日付**: 2026-11-14
- **対象パッケージ**: `fxg-protocol`, `fxg-pty`, `fxg-hub`, `fxg-node`, `ui`
- **対象スクリプト / 設定**: `cargo test -p fxg-pty -p fxg-protocol`,
  `cargo test -p fxg-node`, `pnpm --dir ui run check`,
  `pnpm --dir ui exec vitest run`, `pnpm --dir ui run lint`,
  `pnpm --dir ui run build`, `mise run types`, `mise run fmt`
- **関連ドキュメント**:
  - [`docs/proposals/2026-11-14-workspace-dock.md`](../../proposals/2026-11-14-workspace-dock.md) §3.4, §7
  - [`docs/proposals/2026-11-14-terminal-workspace-scope.md`](../../proposals/2026-11-14-terminal-workspace-scope.md) §3.1
  - [`docs/03-protocol-and-api.md`](../../03-protocol-and-api.md) §3.3
  - [`docs/07-web-ui-and-pwa.md`](../../07-web-ui-and-pwa.md) §4

## 背景

workspace スコープのターミナルは 1 workspace 1 シェルだった。別ディレクトリで
作業したい、開発サーバーを別シェルで動かしたい、といった用途では、同じ
workspace のシェルを共有するしかなかった。ドックの Files は可変タブを持つが、
Terminal は固定 1 シェルのままだった。

## 変更

- **workspace + terminal ID を PTY の同一性にした**:
  - `PtyClientMessage::Open` / `HubToNodeMsg::PtySpawn` / `PtyOpenParams` に
    `terminal_id`（クライアント生成のタブ ID）を追加した。
  - `PtySessionManager` の workspace マップを `(workspace, terminal_id)` キーに
    変更した。同じ組は attach、違う組は新しいシェルになる。
  - ハブの PTY ルートも `terminal_id` で照合する。
  - `terminal_id` 省略時は従来どおり単一ターミナルとして扱う。
- **Terminal ツールにシェルタブを追加**:
  - `+` で新しいシェル、× / 中クリックで閉じる。閉じるときは `kill` を送る。
  - アクティブなタブだけを表示し、非アクティブのタブは hidden でマウントして
    attach を維持する（外側ツールの切替でも detach しない）。
  - タブ構成とアクティブタブは workspace ごとに保持し、リロード後は同じ
    `terminal_id` で attach + replay する。
  - 上限は 8。ラベルは作成時の連番 (`Terminal 1`, `Terminal 2`, ...) で固定し、
    タブを閉じても他のタブの名前は変えない。

## 不変条件

- 同じ `(workspace, terminal_id)` は常に 1 本の PTY。開き直しは attach。
- タブを閉じたときだけ、そのタブの PTY を kill する。ソケット切断・タブ切替・
  ドックを閉じる操作では kill しない。
- デスクトップとモバイルのスロットは引き続き片方だけをマウントする。
- `terminal_id` を省略する旧クライアントは、従来の単一ターミナルとして動く。

## 検証

- `fxg-pty` に「同じ workspace でも terminal ID が違えば別シェル」「同じ ID は
  attach する」テストを追加した。
- `fxg-protocol` の PTY メッセージ roundtrip に `terminal_id` を追加した。
- UI の dock store にタブの追加 / 切替 / 閉じる / 上限 / 永続化 / 旧データ復元の
  テストを追加した。

## 既知の制約

- タブの並べ替え・ピン留めは未対応。
- シェル名（bash / zsh など）は取得せず、連番ラベルを使う。
- タブを閉じる確認ダイアログは出さない（実行中ジョブも終了する）。
