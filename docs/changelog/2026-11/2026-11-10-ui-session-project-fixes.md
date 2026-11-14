# UI とセッション/プロジェクト API の修正

- 日付: 2026-11-10
- 対象パッケージ: `ui/`, `fxg-protocol`, `fxg-hub`, `fxg-node`, `fxg-db`
- 対象スクリプト: `pnpm --dir ui check` / `pnpm --dir ui build`,
  `cargo check --workspace`, `cargo clippy --workspace --all-targets`,
  `cargo test --workspace`, `mise run types`, `mise run fmt`

## 背景

Web UI を一通り操作して見つかった不具合・不足を修正した。UI のみで完結する
ものを先に、API 追加が必要なものを後に実施し、修正単位でコミットしている。

## UI

- 全ページに `<title>` を追加（`/` 以外が空で、SvelteKit のルート
  アナウンサーが "untitled page" を読み上げていた）。
- セッション画面をモバイル幅で修正。ヘッダーのメタ情報と
  Chat/Diff/Terminal 切り替えが幅を奪い合い、1 文字幅の縦積みになっていた。
  小画面では切り替えを別行にし、補助メタ情報を省略する。
- 設定検索を全タブ横断に変更。従来は Bootstrap タブの行しか絞り込まず、
  他タブでは無反応だった。`lib/settings-search.ts` の索引から候補を出し、
  選択で該当タブへ移動する。
- placeholder のみだった入力（サイドバー/設定検索、シークレット、
  検索、両コンポーザ、エージェントのノード選択）に `aria-label` を付与。
- Diff/Terminal パネル内の重複タブと PTY の生 UUID 表示を削除。
  ターミナル左端の文字欠けを padding で解消。
- 文言の統一: `Kill` → `Force stop`、`New here` → `New session here`、
  送信ショートカットに応じたプレースホルダ、`node.db`/FTS5 の内部表現を
  UI から除去、Inbox の件数を見出しの隣へ移動、モバイルの設定タブを折り返し。
- フォルダ追加ダイアログのパンくず `//`、プロジェクト ID/パスの途中折返し、
  `Worktree` ボタン名を修正。
- エージェント選択に ID を表示し、`opencode` と `opencode-v2` を区別。

## API / サーバー

- `POST /api/v1/nodes/:node_id/projects/unlink` を追加。ノード上のフォルダ
  紐付けを削除し、最後の紐付けなら論理プロジェクトも破棄する（セッションは
  保持）。`fxg-protocol` の型、`NodeRpcRequest::UnlinkProject`、ノード実装、
  ハブ中継、監査ログ、UI の削除確認ダイアログまで一通り実装。
- `POST /api/v1/sessions/:id/title` を追加。`SessionTitleChanged` を記録して
  `sessions.title` を更新し、ハブ・他クライアントへ同期する。0〜200 文字の
  検証付き。UI のセッションメニューに Rename を追加。
- 監査アクション `session_rename` を追加。

## 確認

- ブラウザ（Chrome + browse 拡張）で全ルートを表示し、タイトル・
  コンソールエラー 0 件・モバイル幅での横スクロール無しを確認。
- プロジェクト削除とセッションリネームは API (`curl`) と UI の両方で実行し、
  一覧・ヘッダー・サイドバー・ドキュメントタイトルへの反映を確認。
- `svelte-check` 0 errors、`cargo clippy -D warnings` 成功。
