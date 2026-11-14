# 07. Web UI / Android PWA (`ui/`) 設計

PC ブラウザ、ローカル直結 (`node.addr`、既定 `localhost:7860`)、および Android スマートフォン（ホーム画面追加 PWA）のすべてを単一のレスポンシブ SPA で提供します。

---

## 1. フロントエンド技術スタック & 状態管理

- **フレームワーク & ルーティング**: SvelteKit (`Svelte 5` Runes + `@sveltejs/adapter-static` による SPA モード) + Vite + TypeScript
- **パッケージマネージャ & タスクランナー**: `pnpm` + `mise`（`mise.toml` による `specta` 型生成 → UI ビルド → Cargo ビルドの一貫管理）
- **Lint / Format / Test**: `oxlint` / `oxfmt` / `svelte-check` / `Vitest` (+ `happy-dom`)
- **スタイリング & コンポーネント**: Tailwind CSS v4 (`@tailwindcss/vite`) + `shadcn-svelte` (`bits-ui` + モバイルシート用 `vaul-svelte`) + `@lucide/svelte`
- **型安全性**: Rust の `fxg-protocol` から `specta` で自動生成された型定義 (`ui/src/lib/generated/bindings.ts`) をインポートする。
- **状態管理 & 同期**:
  - Svelte 5 Runes (`$state`, `$derived`) を用いた `*.svelte.ts` クラスベースの WebSocket 差分同期ストア。
  - 接続先ストア（ハブ / ローカルノード）の `cursor` を保存し、再接続時に `Subscribe { since_cursor }` で差分再開する。DB 再作成・リセット時はカーソルの巻き戻りを検知してタイムラインを破棄し、0 から全量を再同期する。
  - イベントは `event_id` / `(session_id, node_seq)` で upsert し、`LiveStreamDelta` と永続イベントを `message_id` でマージする。
- **Runes のリアクティブ境界 (`Map` / `Set`)**:
  - `$state` が深く Proxy 化するのは plain object と配列だけのため、時間とともに書き換わるコレクション（`messageDeltas` / `thoughtDeltas` / `toolProgress` / `terminalDeltas` / `pendingPrompts` / `provisioningLogs` 等）は `svelte/reactivity` の `SvelteMap` / `SvelteSet` を使うか、更新のたびに値ごと再代入する（`ui/src/lib/sync/timeline-reactivity.test.svelte.ts` で回帰防止）。

---

## 2. Desk デザインシステムとテーマ

配色を「デザインの一部」ではなく「差し替え可能なテーマ」として分離し、レイアウト・タイポグラフィ・角丸・コンポーネント構造を固定したままパレットを切り替えられる **Desk デザインシステム** を採用しています（正: `ui/src/app.css`）。

- **3 層トークン構成**:
  1. **Design constants**: type scale（11px 大文字ラベル / 12px メタデータ / 13px UI / 14px 本文）、spacing、radius（pill = full / control = 8px / surface = 12px / dialog = 14px）、border（1px ヘアライン主体）、layout、motion。
  2. **Semantic roles**: `--canvas` / `--surface` / `--sidebar-bg` / `--ink` / `--muted-ink` / `--line` / `--chip` / `--signal`（選択面の tint やフォーカスリング用）/ `--ok` / `--warn` / `--danger`。
  3. **shadcn mapping**: `--background` / `--card` / `--border` / `--ring` / `--primary` 等を semantic role へ写像。
- **プリセット**: **Bone**（暖色ニュートラル + 琥珀、既定）、**Graphite**（冷色ニュートラル + コバルト）、**Moss**（緑寄りニュートラル + モス）の 3 種（各 light / dark）。端末ごとの `localStorage["fxg:palette"]` に保存し、サーバーへは同期しない。
- **表示規則**: 機械的文字列（branch / path / ID / model / 時刻）のみ mono フォント。ステータスは色だけでなくラベル・アイコンを併記する。ターミナルと Shiki コードテーマは可読性優先で固定テーマを維持する。
- **選択・状態の表現**: 選択は `signal-soft` の塗りと文字ウェイト、要対応はインラインのステータスアイコンとピル、カードや折りたたみは 1px の全面ボーダーで示す。左端のアクセントバーは使わない。

---

## 3. URL ルーティングと主要画面構成

### 3.1 URL ルーティング契約

| パス                              | 用途                                                                        |
| :-------------------------------- | :-------------------------------------------------------------------------- |
| `/`                               | 新規セッション作成（統一コンポーザー）                                      |
| `/projects`                       | 論理プロジェクト & 各ノードの Git Worktree 一覧・作成・削除                 |
| `/search`                         | FTS5 全文検索（3 文字以上または LIKE フォールバック）                       |
| `/sessions/[id]`                  | セッション詳細（チャット・2 段階 Diff・Web ターミナル）                     |
| `/settings/hub/[tab]`             | Hub スコープ設定（`general`, `process`, `tokens`, `provisioners`, `audit`） |
| `/settings/nodes/[node_id]/[tab]` | Node スコープ設定（`general`, `process`, `pairing`, `agents`, `audit`）     |
| `/settings/browser/[tab]`         | Browser スコープ設定（`appearance`, `notifications`）                       |

Settings 画面の中間パス（`/settings`, `/settings/hub`, `/settings/nodes`, `/settings/nodes/[node_id]`, `/settings/browser`）のみデフォルト先へ `replaceState: true` で補完リダイレクトし、タブ・スコープ切替は通常のブラウザ履歴に積む。利用不可なスコープやオフラインノードは画面内に案内を表示する。

### 3.2 主要画面仕様

1. **サイドバー & 注意喚起（Needs attention）**:
   - 専用の承認 Inbox 画面は持たず、`SessionSummary.status`（`waiting_permission` / `waiting_input` / `error`）を単一ソースとして、サイドバー上部の「Needs attention」・プロジェクト別件数バッジ・各セッション行のステータスアイコン＆ピルで強調表示する。
   - 承認・質問への回答は該当セッションのチャット内インラインカードで行う。
   - デスクトップでサイドバーを閉じても、左端に全高のレールは残さない。セッション詳細ではヘッダー左の再表示ボタン、それ以外の画面ではメイン領域左上に重ねた再表示ボタン（未対応件数バッジ付き）が担い、同時に開閉ボタンが 2 つ表示されることはない。
2. **セッション詳細（チャット & ワークスペース）**:
   - **ヘッダー**: タイトル、実行ステータス、コンテキストピル（実行ノード・ブランチ）のみ常時表示し、詳細メタデータ（node 接続状態・作業ディレクトリ・agent・mode・権限ポリシー・使用量・ID・Fork 元）は「Session details」ポップオーバーに集約。
   - **チャットタイムライン**:
     - `provisioning` 状態時は「Provisioner Log」カードに `stderr` をリアルタイム表示。
     - `LiveStreamDelta` を逐次描画し、確定イベントで置換。思考プロセスはストリーミング中のみ自動展開し、確定後に自動折りたたみ（ユーザーの手動開閉を優先）。
     - 連続する思考・ツール実行・サブエージェント・端末出力・Plan は「作業ブロック」として 1 行に集約・折りたたみ（承認・入力要求・通知は折りたたまず常に表示）。
     - サブエージェントは `SubagentUpdated` と `subagent_id` から親タイムライン内にツリー・インラインカードとして再構築する。
     - 境界行のアクションから Copy / Fork from here を実行可能（Fork 先ではコピー履歴末尾に元セッションへのリンクを表示）。
   - **Diff ペイン（右レール / モバイルシート）**:
     - **This Session**: セッション内でエージェントが編集した差分（ノードがオフラインでも閲覧可）。
     - **Worktree Diff**: ノードへのリアルタイム問い合わせ（`vs Base (main...HEAD)` / `vs HEAD (Uncommitted)`）。PC は `monaco-editor`、モバイルは `shiki`。
   - **Terminal ペイン（右レール / モバイルシート）**: 統合 Web PTY（§4）。
3. **統一コンポーザー**:
   - 新規セッション画面とセッション詳細画面で同一コンポーザーを使用。「文脈行（新規のみ）→ 入力 → セレクタ行（Agent / Mode / Model / その他）→ アクション行（添付 / 自動承認 / その他 / 中断 / 送信）」の順を固定し、未存在項目は非表示にするだけで位置を動かさない。
   - セッション詳細の入力欄は既定 2 行（内容に応じて自動伸長、上限あり）とし、会話タイムラインへ縦スペースを優先配分する。
   - 入力途中のプロンプトはセッション単位（新規セッション画面は画面単位）の下書きとして保持し、別画面へ移動して戻っても復元する。テキストのみ端末の `localStorage` に保存し、添付はメモリ内に保持する。送信に成功した下書きは破棄する。
   - 送信時、ネイティブ復元対応エージェントであれば停止済みセッションでも自動再開（Auto Resume on Send）して送信する（非対応時はメニューの Resume から履歴 Replay）。
4. **Problem center & 接続スイッチャー**:
   - ブラウザで収集したエラー / 警告はサイドバー下部・モバイルヘッダーの件数バッジから開く **Problem center** シートに集約し、レイアウトを押し下げない。
   - 接続先バッジは Hub 接続時・Standalone Node 直結時（`Standalone` 表示）を区別し、緊急停止（Kill Switch）は Hub 設定の Process タブに配置する。

---

## 4. 統合 Web ターミナル (`ITerminalAdapter`)

Web ターミナルは `ghostty-web`（WASM 版 `libghostty-vt` + Canvas 2D）を標準採用し、UI と実装を疎結合にするため `ui/src/lib/components/terminal/types.ts` の `ITerminalAdapter`（`mount` / `clear` / `write` / `onData` / `onResize` / `fit` / `focus` / `dispose`）を介して利用します。

- ノードの ConPTY / Unix PTY と `/api/v1/pty/ws` で直結し、`TerminalView.svelte` が `$effect` で `sessionId` 変化を監視して再接続する。
- モバイル向けに仮想キーバー（`Ctrl`, `Esc`, `Tab`, 矢印キー等）を提供。
- ノード側で `allow_remote_pty = false`（ハブ経由時）の場合は、ローカル端末（`localhost:7860` / CLI）からの利用を案内するフォールバック表示を行う。

---

## 5. Android PWA・Window Controls Overlay・Web Push (VAPID)

- **`manifest.webmanifest`**: `"display": "standalone"`, `"display_override": ["window-controls-overlay", "standalone"]` を設定。
- **Window Controls Overlay (WCO)**: インストール済みデスクトップ Chromium では `env(titlebar-area-*)` と `.wco-*` クラス（`ui/src/app.css`）により OS タイトルバーとアプリトップバーを統合する。
- **Service Worker (`ui/src/service-worker/index.ts`)**:
  - `$app/manifest` と `$app/env` を用いた App Shell キャッシュ（`/api/**` は常にネットワーク素通し）。
  - **Web Push (VAPID)**: ハブ（`<data>/hub/vapid.json` / `hub.db`）とノード単体（`<data>/node/vapid.json` / `node.db`）がそれぞれ独立して `push_subscriptions` を管理する（RFC 8291 / 8292）。
  - **通知種別**:
    - 承認リクエスト: `[Approve]` / `[Reject]` アクションボタン付き（Service Worker からバックグラウンドで `POST /api/v1/sessions/:id/permissions/:req_id/respond` を実行。`ALREADY_RESOLVED` は正常終了扱い）。
    - 質問 (`elicitation`): ボタンなし、タップでセッションを開く。
    - ターン完了: `Running` 観測後の `Idle` / `Error` 遷移時に通知（`Running` 未観測や resync 再生時は送らない）。
  - 失効購読（`404` / `410`）は送信時に自動削除する。
