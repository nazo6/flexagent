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
- **`$effect` の使い分け**:
  - `$effect` は DOM・タイマー・ネットワーク・`localStorage` など外部系への副作用に限定する。
  - prop をローカル state に写す、既定値を選ぶ、URL パラメータを反映するといった「値から値を作る」処理は `$derived` と明示的な上書き状態で表す。effect で「適用済みフラグ」を立てる初期化も行わない。
  - effect が読んだ state を同じ effect で書かない。避けられない場合は `untrack` で読み取り依存を切る。ストアのアクション（`setActiveSession` など）は内部で `untrack` し、呼び出し元の effect に意図しない依存を作らない。

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

| パス                              | 用途                                                                            |
| :-------------------------------- | :------------------------------------------------------------------------------ |
| `/`                               | 新規セッション作成（統一コンポーザー）                                          |
| `/projects`                       | 論理プロジェクト & 各ノードの Git Worktree 一覧・作成・削除                     |
| `/search`                         | FTS5 全文検索（3 文字以上または LIKE フォールバック、`?seq=` で該当イベントへ） |
| `/sessions/[id]`                  | セッション詳細（チャット・ワークスペースドック）                                |
| `/settings/hub/[tab]`             | Hub スコープ設定（`general`, `process`, `tokens`, `provisioners`, `audit`）     |
| `/settings/nodes/[node_id]/[tab]` | Node スコープ設定（`general`, `process`, `pairing`, `agents`, `audit`）         |
| `/settings/browser/[tab]`         | Browser スコープ設定（`appearance`, `notifications`）                           |

Settings 画面の中間パス（`/settings`, `/settings/hub`, `/settings/nodes`, `/settings/nodes/[node_id]`, `/settings/browser`）のみデフォルト先へ `replaceState: true` で補完リダイレクトし、タブ・スコープ切替は通常のブラウザ履歴に積む。利用不可なスコープやオフラインノードは画面内に案内を表示する。

### 3.2 主要画面仕様

1. **サイドバー & 注意喚起（Needs attention）**:
   - 専用の承認 Inbox 画面は持たず、`SessionSummary.status`（`waiting_permission` / `waiting_input` / `error`）を単一ソースとして、サイドバー上部の「Needs attention」・プロジェクト別件数バッジ・各セッション行のステータスアイコン＆ピルで強調表示する。`running` は signal 色の回転インジケータで実行中を常時示し、`idle` とは区別する。完了したエージェントメッセージを未読のまま別画面にいるセッションは signal 色のドットで示す。
   - 未読・既読は端末ローカル（`localStorage`）の状態で、ノード / ハブへは同期しない。ノードのオンライン状態は通常行では緑ドットを出さず、ノード停止・切断など利用できない場合のみ表示する。
   - 承認・質問への回答は該当セッションのチャット内インラインカードで行う。
   - 一覧は `GET /api/v1/sessions` のカーソルページングを使い、既定は最新100件。フィルタと `limit` はサーバー側で適用され、`project_cursors` はページに含まれるプロジェクトだけを返す。各プロジェクトの「Load older sessions」で、そのプロジェクトの古いページだけを追記する（応答の `project_cursors` を使う）。セッションがまだ読み込まれていないプロジェクトは「Load sessions」で先頭ページを取得するが、`ProjectSummary.has_sessions` が `false` のプロジェクト（登録済みでセッションが 1 件も無い）は読み込むものが無いため「Start a session」を出す。最初のページはイベント反映時に再取得し、読み込み済みの古いページは保持する（更新されたセッションは `updated_at` が進むため先頭ページに現れる）。
   - デスクトップでサイドバーを閉じても、左端に全高のレールは残さない。セッション詳細ではヘッダー左の再表示ボタン、それ以外の画面ではメイン領域左上に重ねた再表示ボタン（未対応件数バッジ付き）が担い、同時に開閉ボタンが 2 つ表示されることはない。
2. **セッション詳細（チャット & ワークスペース）**:
   - **ヘッダー**: タイトル、実行ステータス、コンテキストピル（実行ノード・ブランチ）、利用上限アイコンのみ常時表示し、詳細メタデータ（node 接続状態・作業ディレクトリ・agent・mode・権限ポリシー・使用量・ID・Fork 元）は「Session details」ポップオーバーに集約。利用上限は数値を常時出さず、デスクトップはトップバー、モバイルはコンテキスト行にエージェントアイコンだけを置く。アイコンは `AgentSummary.icon`（エージェントカタログ）から取得し、停止中でも消さない。usage が未取得のときはポップオーバーで再開を促す。source（例: `OpenCode Go` / `Antigravity`）・group・window はポップオーバーで示し、アイコンの title / aria-label に `OpenCode Go · 1m 15% left` のような要約を入れる。
   - **チャットタイムライン**:
     - `provisioning` 状態時は「Provisioner Log」カードに `stderr` をリアルタイム表示。
     - `LiveStreamDelta` を逐次描画し、確定イベントで置換。思考プロセスはストリーミング中のみ自動展開し、確定後に自動折りたたみ（ユーザーの手動開閉を優先）。
     - 確定イベント由来のアイテム列はイベント列が変わったときだけ再構築し、ストリーミング差分はその末尾に重ねる。差分ごとに全会話を作り直さないため、長いセッションでもチャンク受信で他メッセージが再レンダリングされない。
     - 連続する思考・ツール実行・サブエージェント・端末出力・Plan は「作業ブロック」として 1 行に集約・折りたたみ（承認・入力要求・通知は折りたたまず常に表示）。
     - 端末カードはコマンドとライブ出力を表示する。コマンドは開始イベントで確定するため、実行中も ID ではなくコマンド名が出る。**実行中の端末出力は折りたたまず**、停止ボタンを出したまま残す。停止は `kill_terminal` でそのコマンドだけを終了し（ターンやセッションは止めない）、終了すると exit code バッジに変わる。
     - 履歴は `before_cursor` で最新ページから読み込み、上部の「Load earlier messages」で古いページを追加する。セッション先頭のページ（capabilities / provisioning log などセッション全体のメタデータ）は別に保持し、会話のページングが先頭へ到達した時点で表示に統合する。開いていないセッションのイベントは保持上限を超えると古いものから破棄し、開いたときに再取得する。
     - **ターン間の移動**: タイムライン右端に常時表示するターンレール（`TurnRail`）が、レンダリング済みの高さに比例した位置へターン開始のティックを置き、いま読んでいるターンだけ色を変える。表示範囲を囲う枠は持たず、スクロールバーと役割が重ならないようにする。未解決の承認・質問・エラー通知は点で示す。クリックでそのターン先頭へ、ドラッグでスクラブし、上端のシェブロンは未読み込みの古い履歴（追加読み込みのみ）。ヘッダーの Turns ボタンで開くターン目次（`TurnOutline`）は本文に重ねるオーバーレイで、発言の先頭行と要対応バッジを一覧し、スクロール位置に追従する。上部の検索欄は `session_id` を付けた `GET /api/v1/search` でそのセッションだけを検索し、ヒットを選ぶとタイムラインの該当イベントへ移動して点滅させる（古いページは必要に応じて追加読み込みする）。ターンは `user` イベントで区切って作る（`ActivityBlock` で畳まれる作業を単位にしない）。
     - 本文の Markdown リンクが workspace 内のファイルを指す場合（相対 / 絶対パス、`file:` URI、Windows ドライブ / UNC）は、新しいタブではなく Files ツールのプレビューで開く（プレビュータブとして開き、既存のプレビュータブを置き換える）。`#L12` / `:42` の行番号は該当行へスクロールして強調し、Markdown ファイルは Source 表示を既定にする。**直接ノードへ接続している場合のみ**で、ハブ経由や workspace 外のパスはリンクの既定動作を変えない。
   - `native_features` に `revert` があるエージェントでは、メッセージ境界の
     「Revert to here」を表示する。revert は `SessionReverted` を境界として
     区間をタイムラインから隠し、それ以降の新しいターンは表示する。
     - サブエージェントは `SubagentUpdated` と `subagent_id` から親タイムライン内にツリー・インラインカードとして再構築する。
     - 境界行のアクションから Copy / Fork from here を実行可能（Fork 先ではコピー履歴末尾に元セッションへのリンクを表示）。
   - **Workspace Dock（右レール / モバイルオーバーレイ）**:
     - 右ペインは workspace（`node_id` + canonical `local_path`）スコープのツールドック。外側のツールは **Files / Changes / Terminal の固定**で、切替はデスクトップのレールとモバイルのセッションヘッダーのアイコンで行う。アクティブツール・Files のツリー表示・開いているファイル・プレビューのモード・幅を workspace ごとに保持し、セッション切替では閉じない。workspace が変わったときだけ状態を入れ替える。
     - ドックのヘッダーはアクティブツール名と閉じるボタンだけを持ち、外側の可変タブは持たない。ドックを閉じるのはヘッダー / オーバーレイの閉じるボタンで、ツールを閉じる概念はない。
     - モバイルではセッションヘッダーの Files / Changes / Terminal アイコンから 1 タップで開く。会話の上に背景を暗くしたオーバーレイとして表示し、背景タップまたは右上の閉じるボタンで畳む。会話は背後に残すが `inert` にして操作対象から外す。
     - **Files**: workspace の Git toplevel（非 Git なら workspace パス）を起点にした遅延読み込みのツリーと、ファイルプレビューの 2 ペイン。`git status` の変更バッジ、隠しファイル / 重いディレクトリの表示切替、変更のみの一覧を持つ。ツリー上部のフィルタは `GET .../workspace/fs/search` による **ファイル名のクイックオープン** で、展開していないディレクトリのファイルも相対パスの部分一致で探してプレビューで開ける（Git では `git ls-files` の tracked + untracked - ignored、非 Git では上限付きの走査）。デスクトップではツリーを隠してプレビューを全幅にでき、分割比は境界のドラッグで変更できる（workspace ごとに保持）。モバイルではツリー ↔ プレビューを戻るボタンで切り替える。ハブ経由でも利用できる（ハブが対象ノードへ中継する）。
     - **File preview（Files 内）**: プレビュー上部にファイルタブ（可変・中クリックで閉じる）を持つ。タブには **プレビュー（一時）** と **固定** の 2 種があり、プレビュータブは常に最大 1 枚で、次のプレビューオープンがその位置を置き換える（タブは増えない）。ツリー / 変更一覧 / チャットのファイルリンクの単クリックはプレビュー、ダブルクリックは固定で開き、タブのダブルクリックでも固定できる（プレビューのヘッダーのピンボタンでも固定できる）。同じファイルを開き直したときはタブを増やさず内容と行を更新し、固定で開き直すとプレビューは固定へ昇格する。プレビュー状態は workspace ごとに保存・復元する。`View` は Shiki の読み取り専用表示（Markdown は Preview / Source を切替、PNG / JPEG / GIF / WebP / BMP / AVIF / ICO は画像プレビュー）、`Diff` は単一ファイルの diff（PC は `monaco-editor`、モバイルは `shiki`）。Git status バッジを表示し、バイナリ・大きすぎるファイル・削除済みファイルを区別する。**Phase 1 は読み取り専用**で、編集・保存は未対応（[`docs/backlog.md`](./backlog.md) §6）。
     - **Changes**: `Session`（セッション内の tool diff）/ `Worktree`（`git status`）/ `Branch`（base との累積 diff）の 3 スコープ。行をクリックすると Files ツールのプレビューにファイルを開く（Diff モード）。Session の編集差分はノードがオフラインでも閲覧できる。`Worktree` スコープでは変更一覧の下の入力欄から、worktree の全変更を `git add -A` で stage して 1 コミットを作成できる（メッセージ必須、部分 stage は対象外）。ヘッダーの Pull / Push から upstream と同期できる。Pull は fetch + `git merge --ff-only` のみで、merge commit の作成や conflict の解消は行わない。Push は force せず、upstream が未設定なら `origin` に初回 push して upstream を設定する。
     - **Terminal**: workspace の統合 Web PTY（§4）。タブごとに独立したシェルを持ち、`+` で追加、× / 中クリックで閉じる。ドックが開いている間は非アクティブのタブもマウントを維持し、PTY の attach を保つ。
3. **統一コンポーザー**:
   - 新規セッション画面とセッション詳細画面で同一コンポーザーを使用。「文脈行（新規のみ）→ 入力 → セレクタ行（Agent / Mode / Model / その他）→ アクション行（添付 / 自動承認 / その他 / 中断 / 送信）」の順を固定し、未存在項目は非表示にするだけで位置を動かさない。添付チップの一覧と画像添付ボタンは `ComposerAttachments` / `ComposerAttachImageButton` を共有し、2 画面で同じ見た目・削除・再選択の挙動にする。
   - セッション詳細の入力欄は既定 2 行（内容に応じて自動伸長、上限あり）とし、会話タイムラインへ縦スペースを優先配分する。
   - 入力途中のプロンプトはセッション単位（新規セッション画面は画面単位）の下書きとして保持し、別画面へ移動して戻っても復元する。テキストのみ端末の `localStorage` に保存し、添付はメモリ内に保持する。送信に成功した下書きは破棄する。
   - 送信時、ネイティブ復元対応エージェントであれば停止済みセッションでも自動再開（Auto Resume on Send）して送信する（非対応時はメニューの Resume から履歴 Replay）。
4. **Problem center & 接続スイッチャー**:
   - ブラウザで収集したエラー / 警告はサイドバー下部・モバイルヘッダーの件数バッジから開く **Problem center** シートに集約し、レイアウトを押し下げない。
   - 接続先バッジは Hub 接続時・Standalone Node 直結時（`Standalone` 表示）を区別し、緊急停止（Kill Switch）は Hub 設定の Process タブに配置する。

---

## 4. 統合 Web ターミナル (`ITerminalAdapter`)

Web ターミナルは `ghostty-web`（WASM 版 `libghostty-vt` + Canvas 2D）を標準採用し、UI と実装を疎結合にするため `ui/src/lib/components/terminal/types.ts` の `ITerminalAdapter`（`mount` / `clear` / `write` / `onData` / `onResize` / `fit` / `focus` / `dispose`）を介して利用します。

- ターミナルのスコープは **workspace**（`(node_id, session.local_path)`）と
  **terminal ID** の組です。セッションはターミナルの所有者ではなく**origin**で、
  同じ worktree の別セッションへ切り替えても接続は維持されます（`TerminalView.svelte`
  は `sessionId` ではなく workspace + terminal ID の変化で再接続します）。
- `/api/v1/pty/ws` の `open` は workspace + terminal ID に対して冪等で、既存の
  シェルがあれば attach します。タブを増やすと terminal ID が変わるため、同じ
  workspace に独立したシェルが複数できます。ドックを閉じる・リロードすると socket
  は閉じますが、ノード側は **detach** するだけでシェルは残ります（無操作 TTL の
  既定は 30 分）。同じ workspace のセッション切替では socket を維持します。
- attach 時は保持済み出力（リングバッファ + シーケンス番号）が先に送られるため、開き直すと scrollback が復元されます。UI は再描画の重複を避けるため、再接続前に端末バッファを clear します。
- **1 表示 1 PTY**: ドックはデスクトップ用（分割ペイン）とモバイル用（全画面）の
  スロットをビューポートで切り替えて**片方だけ**マウントします。両方をマウント
  すると同じ `terminal_id` の `TerminalView` が 2 つ動き、同じシェルへ 2 つの
  socket が attach して resize とフォーカスが競合します。ターミナルのタブ自体は
  複数持てますが、各タブの表示は 1 つだけです。
- **タブ切替と detach**: ドックが開いている間、非アクティブのターミナルタブは
  `hidden` でマウントしたままにし、タブ切替や外側ツールの切替で PTY を detach
  しない。再アクティブ時に `fit()` と resize を送る。タブを閉じたときは `kill`
  を送る。ドック自体を閉じたときだけ unmount → detach し、再度開くと `open` が
  冪等に attach + replay する。
- **IME 入力の不変条件**: `ghostty-web` は入力用の隠し `textarea` ではなく `contenteditable` なラッパーにフォーカスし、ラッパーで `beforeinput` を打ち消す（`textarea` からバブリングする分も止まる）上に、変換中 (`isComposing` / `keyCode === 229`) のキーをすべて破棄する。このため `GhosttyWebAdapter` がフォーカスを `textarea` へ移し、`beforeinput` を `textarea` で止めて挿入を許可したうえで、挿入されたテキストを `input`（`compositionend` が先に来る場合の取りこぼしも）から 1 回だけ送る。変換中テキストはカーソルセルに自前で描画する。
- ウィンドウ幅がデスクトップ / モバイルの境界（`64rem`）をまたいで再マウントされても、dispose は detach のみなので PTY は作り直されません。
- モバイル向けに仮想キーバー（`Ctrl`, `Esc`, `Tab`, 矢印キー等）を提供。
- ノード側で `allow_remote_pty = false`（ハブ経由時）の場合は、ローカル端末（`localhost:7860` / CLI）からの利用を案内するフォールバック表示を行う。

---

## 5. Android PWA・Window Controls Overlay・Web Push (VAPID)

- **`manifest.webmanifest`**: `"display": "standalone"`, `"display_override": ["window-controls-overlay", "standalone"]` を設定。
- **Safe area**: `app.html` の `viewport-fit=cover` に対し、`app.css` の `--safe-*`（`env(safe-area-inset-*)`）を `.safe-area-shell`（アプリ全体の上下左右）と `.safe-area-portal`（body へ portal されるモバイルドロワー）に適用する。ノッチ・ステータスバー・ホームインジケータの下にヘッダー / コンポーザー / ナビゲーションを置かない。トーストも上端のインセット分だけ下げる。
- **Window Controls Overlay (WCO)**: インストール済みデスクトップ Chromium では `env(titlebar-area-*)` と `.wco-*` クラス（`ui/src/app.css`）により OS タイトルバーとアプリトップバーを統合する。
- **Service Worker (`ui/src/service-worker/index.ts`)**:
  - `$app/manifest` と `$app/env` を用いた App Shell キャッシュ（`/api/**` は常にネットワーク素通し）。
  - **Web Push (VAPID)**: ハブ（`<data>/hub/vapid.json` / `hub.db`）とノード単体（`<data>/node/vapid.json` / `node.db`）がそれぞれ独立して `push_subscriptions` を管理する（RFC 8291 / 8292）。
  - **通知種別**:
    - 承認リクエスト: `[Approve]` / `[Reject]` アクションボタン付き（Service Worker からバックグラウンドで `POST /api/v1/sessions/:id/permissions/:req_id/respond` を実行。`ALREADY_RESOLVED` は正常終了扱い）。
    - 質問 (`elicitation`): ボタンなし、タップでセッションを開く。
    - ターン完了: `Running` 観測後の `Idle` / `Error` 遷移時に通知（`Running` 未観測や resync 再生時は送らない）。
  - 失効購読（`404` / `410`）は送信時に自動削除する。

### 5.1 非セキュアオリジン（HTTP）での縮退動作

ブラウザの **Secure Context** 限定 API は、`localhost` / `127.0.0.1` 以外の
`http://` オリジンでは利用できません。FlexAgent の Web UI は LAN / Tailscale の
HTTP アドレスで直接開かれることが多いため、使えない機能を明示しつつ、
それ以外の基本機能は HTTP でも動作させます。

- **HTTP でも動作する**: 認証（Bearer / `fxg_session` Cookie）、セッション一覧・
  チャット、Web PTY、ファイル差分、設定、接続先切り替え。
- **HTTP では制限される**（ブラウザの仕様であり、サーバー側の制限ではない）:
  - Service Worker と PWA インストール（オフライン App Shell を含む）
  - Web Push の購読・通知、Notification API
  - `navigator.clipboard`（非同期 Clipboard API）
  - `crypto.randomUUID()`
- **代替実装**:
  - UUID が必要な箇所は `crypto.getRandomValues()` から v4 UUID を生成する
    (`ui/src/lib/uuid.ts`)。
  - コピー操作は `navigator.clipboard` が無い / 拒否された場合に、一時的な
    textarea と `document.execCommand("copy")` へフォールバックする
    (`ui/src/lib/clipboard.ts`)。コピーはユーザー操作（クリック）を起点に
    実行するため、レガシー経路でも成功する。
- 通知パネル（**設定 > ブラウザ > 通知**）は、非セキュアオリジンでは
  `Requires HTTPS or localhost (secure context)` と表示して購読ボタンを無効化する。
  Notification API 自体が無い環境では権限を「拒否」ではなく「未要求」として扱う。
- 制限を解除するには、TLS リバースプロキシで HTTPS 配信するか `localhost` で開く。
  HTTP のまま公開する場合も、ハブ / ノードは LAN / VPN の内側に限定する
  （[`docs/01-architecture-and-sync.md`](./01-architecture-and-sync.md) §7）。
