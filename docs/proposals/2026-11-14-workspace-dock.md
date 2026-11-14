# 2026-11-14 右ペインの Workspace Dock 化

- **日付**: 2026-11-14
- **ステータス**: `In Progress`
- **対象パッケージ**: `fxg-protocol`, `fxg-node`, `fxg-hub`, `ui`
- **対象スクリプト / 設定**: `mise run check` / `mise run fmt` / `mise run types` / `pnpm --dir ui test`
- **関連ドキュメント**:
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md) §3.3 (PTY / セッション API)
  - [`docs/05-cli-and-config.md`](../05-cli-and-config.md) §2 (データディレクトリ・ノード設定)
  - [`docs/06-type-safety.md`](../06-type-safety.md) (open enum / 文字列判定の禁止)
  - [`docs/07-web-ui-and-pwa.md`](../07-web-ui-and-pwa.md) §3.2, §4
  - [`docs/proposals/2026-11-14-terminal-workspace-scope.md`](./2026-11-14-terminal-workspace-scope.md)

---

## 0. 結論

- セッション画面の右ペインを、**workspace スコープのツールドック**にする。
  - **workspace**: `(node_id, canonical local_path)`。ターミナルと同じ同一性。
  - **外側のツールは Files / Changes / Terminal の固定**。切替はデスクトップのレールとモバイルのセッションヘッダーのアイコンで行う。
  - **Files はツリー + プレビュー**。プレビュー内に可変ファイルタブを持ち、中クリックで閉じられる。外側に可変タブは持たない。
- ドックの状態（アクティブツール、Files のツリー表示、開いているファイル、モード / スコープ、幅、開閉）は **workspace ごと**に保持し、セッション切替では閉じない。
  - 現在の `activePanel = null` リセットを廃止する。
- ツールは **Files / Changes / Terminal**。File プレビューは Files の中にあり、Changes の行やチャットの tool diff から Files のプレビューにファイルを開く。プレビュー内で `View | Diff` を切り替える。
- ファイルアクセスの境界は **workspace の git toplevel（worktree ならその worktree）配下**。worktree / メイン repo / 別 worktree をまたがない。
- Phase 1 のファイル API は **ローカル接続のみ**。ハブ経由での読み書きは設定で解禁するまで無効にする。
- 段階は Phase 0（UI 基盤）→ Phase 1（閲覧と連携）→ Phase 2（編集）→ Phase 3（更新検知・追加ツール）。

## 1. 現状と課題

現行の右ペインは `ui/src/lib/session-panels.ts` の `diff | terminal` を `SessionPanelHost.svelte` が分岐するだけである。タブ、開いているファイル、ツールごとの状態を持たない。

- `ui/src/routes/sessions/[id]/+page.svelte` がセッション切替時に `activePanel` を `null` に戻すため、workspace スコープにしたターミナルを実質セッション単位で閉じている。
- `DiffPane` はセッションの tool diff と worktree 全体の diff を一括取得する。git status や単一ファイル diff がなく、変更ファイルからファイルを開く導線もない。
- ノードの `fs` API はフォルダピッカー用の `GET /api/v1/nodes/:id/fs/browse` のみで、任意パスのディレクトリ一覧を返す。ファイルの読み書き、workspace 境界の検証、git status はない。
- ツールを増やすたびに `SessionPanelId` とホストの `{#if}` を書き換える構造で、ツール固有の状態を持てない。

## 2. 設計

### 2.1 用語と同一性

- **workspace**: `(node_id, dunce::canonicalize 済みの local_path)`。ドックとターミナルの状態キー。
- **workspace root**: ファイルアクセスを許可するディレクトリ。workspace が Git 作業ツリー内なら `git rev-parse --show-toplevel`、そうでなければ workspace のパス。
- **session**: workspace の来歴。ドックに現在の `session_id` を渡すだけで、ドックの寿命を決めない。
- **access root**: ノードがリクエストごとに解決する、workspace root の実パス。パス検証は常にこの配下で行う。

### 2.2 ツールモデル

- **外側のツール** は 3 つの固定 singleton。
  - `Files`: ツリー + ファイルプレビュー。
  - `Changes`: セッション / worktree / branch の変更一覧。
  - `Terminal`: workspace の PTY。
- **Files 内のファイルタブ** はリソース（絶対パス）ごとに 1 つ。同一パスのタブは再利用し、`mode`（`view` / `diff`）と `scope`（`uncommitted` / `branch_base`）を持つ。
- 外側のツール切替はレール / モバイルヘッダーのアイコンで行う。ドックのヘッダーはアクティブツール名と閉じるボタンだけを持ち、外側の可変タブは持たない。
- ツールはレジストリに定義する。追加はレジストリと描画分岐で完結させ、`SessionPanelId` のような排他的な union を UI 全体に配らない。
- ファイルタブは開いた順に並べる。上限は 8。超えたら最も古いタブを閉じる。並べ替え / ピン留めは Phase 3 以降。

### 2.3 状態と永続化

- ドックの状態はアプリレベルの store に置く。
  - workspace キーごとの `activeTool`、`treeVisible`、`files`（path / mode / scope）、`activeFile`、`collapsed`。
  - ユーザー共通の `width`（既存の割合ベースを `clamp(22rem, N%, 75%)` で使う）。
- `localStorage` に保存するのはツール / ファイルのメタデータと幅だけ。ファイル内容・未保存バッファ・スクロール位置は保存しない。
  - リロード後、File プレビューは再読込し、Terminal は attach + replay で復元する。
- 保存対象は直近の workspace 数件に制限し、削除済みワークスペースのエントリは読み込み時に捨てる。
- workspace が変わったときだけ、保存 → 対象 workspace の状態を読み込む。同じ workspace のセッション移動では何もしない。
- Changes のスコープや Files のツリー表示は workspace ごとに保持し、外側ツールを切り替えても失わない。
- 初回の workspace は畳んだ状態から始める。右ペインを勝手に開かない。

### 2.4 ライフサイクル

- **ドックを開く**: レール / モバイルヘッダーのアイコンでアクティブツールを切り替える。同じツールの再選択はドックを畳む。
- **ドックを閉じる**: ヘッダー / オーバーレイの閉じるボタンで畳む。アクティブツールとファイルタブは保持し、次回は同じ状態で開く。外側ツールを「閉じる」概念はない。
- **ファイルタブを閉じる**: Files 内のタブだけが閉じる。最後のファイルタブを閉じてもドックは畳まず、ツリーと空のプレビューを残す。ターミナルは外側ツールを切り替えても unmount しない。
- **セッション切替**: 同じ workspace ならアクティブツール・ファイルタブ・PTY を維持する。別 workspace なら旧状態を保存し、新 workspace の状態を読み込む。
- **ドックを畳む**: ターミナルは unmount → detach。再度開くと `open` が冪等に attach + replay する。
- **外側ツールの切替**: ドックが開いている間、ターミナルは hidden でマウントしたままにし、WS を維持する。再アクティブ時に `fit()` と resize を送る。
  - 非表示中に 0 サイズで fit しない。active 復帰時に再 fit する。
  - Files / Changes も hidden でマウントを維持し、ツリー・スコープ・選択ファイルを保つ。File プレビューは active なときだけマウントし、store の mode / scope から再読込する。
- **worktree 削除**: 該当 workspace の状態を破棄する。PTY の kill はターミナル側の worktree 連動に従う。
- **ノード切断**: ツールとファイルタブは残し、内容を読み取り専用にする。保存は失敗させ、復帰後に再試行させる。

### 2.5 UI 構成

- **レール（右端）**: Files / Changes / Terminal のアイコン。バッジ（Changes の件数、Terminal の実行中など）を出す。最下部にレール自体を隠すボタン。
- **ドックヘッダー**: アクティブツールのアイコン + 名前 + 閉じるボタン。外側のタブバーは持たない。
- **Files**: 上部ツールバー（更新、隠しファイル / 重いディレクトリ / 変更のみ / ツリー表示の切替、root パス）と、ツリー + プレビューの 2 ペイン。
  - デスクトップはツリーを隠してプレビューを全幅にできる。モバイルはツリー ↔ プレビューを戻るボタンで切り替える。
  - プレビュー上部にファイルタブ（basename、閉じる、中クリック）。その下に `View | Diff`、スコープ、Markdown Preview / Source、更新、Git status を置く。
- **Changes**: スコープ（Session / Worktree / Branch）、更新。行クリックで Files のプレビューにファイルを開く。
- **Terminal**: 接続状態、再接続、新しいシェル。
- **デスクトップ**: チャットとドックの分割。既存のリサイズハンドルを維持する。ドックは `hidden lg:flex`。
- **モバイル**: セッションヘッダーの Files / Changes / Terminal アイコンから**1 タップ**で開く。会話の上に**背景を暗くしたオーバーレイ**としてドックを出し、オーバーレイの外側（背景）のタップ、または右上の閉じるボタンで畳む。会話は背後に残すが、`inert` にして操作・フォーカスの対象から外す。デスクトップとモバイルのスロットはビューポートで切り替え、片方だけをマウントする。
- **ファイルタブのアクセシビリティ**: `role="tablist"` / `role="tab"` / `aria-selected` とし、左右キーで移動できるようにする。
- チャットが主、ドックが従。エージェントの作業中にタブを勝手に切り替えない。更新はバッジと未読ドットで知らせる。

## 3. ツール

### 3.1 Files

- workspace root を起点にした遅延読み込みのツリー。
- ディレクトリを先、ファイルを後に並べる。ソートは名前順（大文字小文字を無視）。
- `.git` は常に隠す。`.gitignore` と既定の重いディレクトリ（`node_modules` / `target` / `dist` など）は隠し、トグルで表示できるようにする。エントリ数には上限を設ける。
- `git status` の変更種別をバッジで表示する（M / A / D / R / U / 競合）。
- 「変更のみ」フィルタを持つ。
- ファイルをクリックすると File タブを開く。既に開いていればそのタブをアクティブにする。
- コンテキスト操作: パスのコピー、Changes で表示（変更がある場合）。ターミナルへの cd は Phase 3（シェル統合が必要）。

### 3.2 Changes

- 一覧は次の 2 系統をマージする。
  - セッションの tool diff（ノードがオフラインでも閲覧できる）。
  - workspace の `git status`（worktree の実ファイル状態）。
- スコープは既存の 3 つを維持する。
  - **Session**: 現在のセッションが編集したファイル。
  - **Worktree: vs HEAD**: 未コミットの変更。
  - **Worktree: vs Base**: base branch との累積差分。
- Session / Branch の +/- は diff の `FileDiff` から、Worktree は `git status` のバッジを表示する。worktree 全体の unified diff は一覧のためには取得しない（numstat は Phase 3）。
- ファイルを選択したときに初めて単一ファイルの diff を取得する。大きなリポジトリでも一覧が軽い。
- 行をクリックすると Files ツールのプレビューを Diff モードで開く。Diff はプレビューのツールバーで切り替えられる。
- ファイルがディスク上に存在しない（削除済み）場合は、diff のみの読み取り専用プレビューとして開く。

### 3.3 File preview（Files 内）

- 1 ファイルタブ = 1 パス。プレビュー内で `Diff | Edit` を切り替える。
- **View**: Shiki の読み取り専用表示。Markdown は Preview / Source を切り替える。大きすぎるファイル・バイナリは案内表示。
- **Diff**: 単一ファイルの unified diff。既存の `DiffViewer` の表示規則を踏襲する。
- **Edit**（Phase 2）:
  - Monaco の通常エディタ。言語は拡張子から解決する（既存の `MonacoDiff` と同じ対応表を使う）。
  - 未保存はファイルタブにドットを出す。`Cmd/Ctrl+S` で保存。
  - 保存は楽観ロック。読み込み時のハッシュが変わっていたら `CONFLICT` とし、上書き・再読込を選ばせる。
  - 外部変更を検知したらバナーで知らせ、保存時に必ず確認する。
  - モバイルの編集は Phase 2 では無効（閲覧のみ）とし、必要になったら対応する。
- 行ジャンプ（チャットの差分から開く）は Phase 3。

### 3.4 Terminal

- workspace ごとに複数のシェルタブを持つ。各タブはクライアント生成の
  `terminal_id` で識別し、ノードは `(workspace, terminal_id)` ごとに PTY を 1 本保持する。
- タブバーの `+` で新しいシェル、× / 中クリックで閉じる。閉じるときは `kill` を送る。
  ツールバーには接続状態と再接続を置く。既存のステータスバッジと仮想キーバーは維持する。
- ドックが開いている間は非アクティブのタブも hidden でマウントし、外側ツールの切替で
  PTY を detach しない。ドックを閉じたときだけ detach する。
- 再接続は同じ `terminal_id` の `open`（冪等）と replay を使う。リロード後も
  タブ構成を復元し、同じシェルへ attach する。

### 3.5 将来のツール

レジストリに追加するだけで、レール / モバイルヘッダーに現れる形にする。

- Workspace search（grep）
- Tests / tasks（プロジェクトのタスクランナー）
- Git log / graph
- Dev server preview
- Problems（ワークスペースの診断）

いずれも workspace スコープを基本とし、セッション固有の情報はコンテキストとして受け取る。

## 4. worktree とファイルアクセスの境界

worktree は独立した workspace として扱う。

- 同一性は canonical path。メイン repo と `<data>/node/worktrees/<project>/<branch>` は別 workspace。
- Files のルートは、その workspace の **git toplevel**。worktree なら worktree のルートで、メイン repo には遡らない。
- `git status` / diff もその worktree で実行する。Changes はその worktree の変更だけを返す。
- パス検証は、登録済みの workspace パスから解決した access root の配下だけを許可する。
  - worktree A のセッションからメイン repo のファイルは開けない。逆も同じ。
  - シンボリックリンクは canonicalize 後に access root の外へ出るものを拒否する。
  - `.git` 配下は一覧・読み書きの対象外。
- 同じ worktree の別セッションは、同じドック・タブ・PTY を共有する。
- 別 worktree / メイン repo のセッションへ移動すると、その workspace のタブセットに切り替わる。ターミナルも別の PTY になる。
- サブディレクトリでセッションを開始した場合は、端末の cwd とドックの identity はそのサブディレクトリのまま。ただし Files の表示ルートは git toplevel に上げ、Changes と一致させる。
- プロジェクトは worktree をまとめる表示上の親に留める。プロジェクト画面での worktree 横断一覧は将来の読み取り集約とし、編集は常に特定 worktree で行う。
- worktree を削除したときは、その workspace のドック状態を破棄する。ノードがパスを見失った場合はタブを閉じ、workspace が無いことを表示する。

## 5. プロトコル / API

型の正は `crates/fxg-protocol` と生成済み bindings とし、ここでは意味だけを定める。

### 5.1 エンドポイント

ノードに workspace 制限付きの API を追加し、ハブは既存の `NodeRpc` と同じ形でプロキシする。

- `GET /api/v1/nodes/{node_id}/workspace/fs/list?path=<dir>`
  - ディレクトリ一覧。エントリは名前・絶対パス・種別・サイズ・更新時刻・隠し属性を持つ。
- `GET /api/v1/nodes/{node_id}/workspace/fs/file?path=<file>`
  - ファイル内容。テキスト / バイナリ / 大きすぎる、を区別して返す。内容ハッシュと更新時刻を含める。
- `PUT /api/v1/nodes/{node_id}/workspace/fs/file`
  - 既存ファイルの保存。パス・内容・期待ハッシュを運ぶ。成功時に新しいハッシュと更新時刻を返す。
- `GET /api/v1/nodes/{node_id}/workspace/git/status?path=<dir>`
  - branch / ahead / behind と、変更エントリ（path, old_path, index/worktree の変更種別, untracked, conflicted）。
- `GET /api/v1/nodes/{node_id}/workspace/git/diff?path=<dir>&scope=<scope>&file=<relative path>`
  - 単一ファイルの diff。`scope` は既存の `DiffScope` を使う。`file` を省略した場合は既存の DiffPane と同じく全体の diff を返す。

既存のセッション diff (`GET /api/v1/sessions/:id/diff`) は、セッションから workspace を解決する導線として残す。新しい Changes は workspace API を優先する。

`NodeRpcRequest` に `WorkspaceFsList` / `WorkspaceFsRead` / `WorkspaceFsWrite` / `WorkspaceGitStatus` / `WorkspaceGitDiff` を追加し、`NodeRpcResponse` に対応する結果を追加する。タイムアウトは既存の既定（30 秒）で足りる。

### 5.2 パス検証 (不変条件)

- リクエストのパスを `dunce::canonicalize` する。存在しないパスは親を canonicalize してから末尾を結合する（書き込みは既存ファイルのみなので、実質は読み取りと同じ）。
- workspace の登録済みパス（`project_node_bindings` / `project_path_links` / `session.local_path`）から access root を解決する。登録の無いパスは対象外。
- 対象は access root の配下でなければならない。`Path::starts_with` を canonical path 同士で使い、文字列の prefix 比較はしない。
- `.git` 配下は拒否する。worktree の `.git` ファイルも同様。
- Windows ではドライブレターの大文字小文字と UNC プレフィックスを考慮する。既存の `dunce::canonicalize` 方針を踏襲する。
- 一覧は 1 ディレクトリあたりのエントリ数に上限を設け、超過時は truncated を返す。
- 読み取りはサイズ上限（既定 2 MiB）を設ける。UTF-8 以外はバイナリとして扱う。
- 書き込みは一時ファイル + rename の原子的更新。パーミッションを維持し、改行コードは読み込み時のものを保持する。
- 書き込みは `fs_write` として監査に記録する。読み取りは記録しない（ノイズになるため）。

### 5.3 リモートアクセス

Phase 1 では **ローカル接続（ループバック）からのみ** ファイル API を許可する。

- ハブ経由のリクエストはノード側で拒否する。ハブのルート自体は用意してよいが、ノードが `FORBIDDEN` を返す。
- UI は接続種別（ハブ / スタンドアロン）とノードの capability で Files ツールを出し分ける。ハブ経由では理由を表示し、ローカル接続を案内する。
- 将来、ノード設定で読み取り / 書き込みを個別に解禁できるようにする。既定は off。設定を追加する際は `docs/05` と `NodeSettings` を更新する。

### 5.4 エラーと更新通知

- 想定内の状態はエラーではなく列挙で返す（バイナリ、大きすぎる、truncated）。
- 想定外は既存の `ErrorCode` に追加する（例: workspace 外パス、読み取り専用、競合、未登録 workspace）。UI は i18n キーとして扱う。
- Phase 1 の更新は、タブのアクティブ化・ドックの表示・手動更新・セッションのターン完了（idle / error）で行う。ファイル監視は Phase 3。
- Phase 3 ではノードのファイル監視を debounce してクライアント WS で通知し、開いているタブを無効化する。書き込み中の競合は保存時に検出する。

## 6. 段階計画

### Phase 0: ドック基盤（バックエンドなし）

- `ui/src/lib/dock/` にツールレジストリを追加する。
- `ui/src/lib/stores/dock.svelte.ts` に workspace キーの状態と永続化を実装する。
- 固定ツールの `DockHost` / `DockRail` を追加する。Files 内のファイルタブは Phase 1 で追加する。
- `session-panels.ts` / `SessionPanelHost.svelte` / `SessionToolRail.svelte` / `SessionToolsSheet.svelte` を削除し、ドックへ一本化する（ラッパーを残さない）。
- セッション画面の `activePanel` と幅のローカル state を削除し、ドック store に移す。
- 既存の Changes / Terminal をツールとして移植する。
- Terminal に active 制御を追加し、非表示中の fit を避ける。ドックを閉じたときだけ detach する。
- モバイルのヘッダーランチャーとオーバーレイを維持する。片肺マウントの不変条件を守る。
- Vitest で store を検証する（workspace 切替、永続化、ファイルタブ上限、復元時の不正エントリ）。
- 受け入れ: 同じ workspace のセッション移動で Terminal が閉じない / リロードでツール状態が戻る / 別 workspace で状態が切り替わる / ドックを閉じてもツール状態が残る。

### Phase 1: 閲覧と連携

- ノード / ハブに workspace fs / git status / 単一ファイル diff API を追加する（ローカル接続のみ）。
- Files ツリーと git status の装飾、変更のみフィルタを実装する。
- Changes を git status + セッション diff の統合一覧にし、単一ファイル diff を遅延取得する。
- Files 内のファイルプレビュー（View / Diff）と、Changes / Files / チャットからの導線を実装する。
- 削除済みファイル・バイナリ・大きすぎるファイル・ノードオフラインの表示を整える。
- 受け入れ: Files から開いたファイルが Changes と同じ変更状態を示す / worktree の外に出られない / ハブ経由では明示的に無効。

### Phase 2: 編集

- 保存 API、楽観ロック、競合 UI、監査を追加する。
- Monaco の編集タブ、未保存ドット、`Cmd/Ctrl+S`、外部変更バナーを実装する。
- モバイルは閲覧のみとし、編集はデスクトップに限定する。
- 受け入れ: 保存後に Changes の状態が更新される / 競合時に上書き・再読込を選べる / 権限の無い接続では保存できない。

### Phase 3: 仕上げ

- ファイル監視と WS 通知（debounce）を追加する。
- チャットの差分から File タブの該当行へジャンプする。
- ファイルタブの並べ替え・ピン留め、workspace search などの追加ツール。
- プロジェクト画面から worktree ごとのドックを開く導線。
- ターミナルのシェル統合（cwd 追跡）と「ここでターミナルを開く」。

## 7. 不変条件

1. ドックの状態キーは `(node_id, canonical local_path)`。プロジェクトやセッションをキーにしない。
2. セッション切替はドックの寿命を決めない。workspace が変わったときだけツール状態を入れ替える。
3. ターミナルは workspace と `terminal_id` の組で一意。同じ組への `open` は既存の
   シェルへ attach し、違う `terminal_id` は独立したシェルになる。
4. ターミナルはドックが開いている間だけ hidden 保持し、ドックを閉じたら detach する。
   ソケット切断やタブ切替では kill しない。タブを閉じたときだけ kill する。
5. デスクトップとモバイルのスロットは片方だけをマウントする。各タブの `TerminalView`
   は最大 1 つの PTY に接続する。
6. ファイル API は登録済み workspace の access root 配下だけを扱う。worktree / メイン repo / 別 worktree をまたがない。
7. Phase 1 のファイル API はローカル接続のみ。ハブ経由は明示的に拒否する。
8. 書き込みは既存ファイルに限定し、期待ハッシュが一致したときだけ成功する。作成・削除・リネームは今回の範囲外。
9. ツール / ファイルのメタデータだけを localStorage に保存する。内容・未保存バッファは保存しない。
10. ツールの追加はレジストリと描画分岐で行い、排他的な `SessionPanelId` を復活させない。

## 8. 受け入れ条件（全体）

- 同じ workspace の別セッションへ移動しても、アクティブツール・開いていたファイルタブ・ターミナルのシェルが残る。
- 別 workspace へ移動すると、その workspace のツール状態に切り替わる。ターミナルもその worktree の PTY になる。
- リロード後、アクティブツールとファイルタブが復元され、Terminal はタブごとに
  attach + replay で scrollback が戻る。
- Terminal の `+` で同じ workspace に独立したシェルを追加でき、× / 中クリックで
  そのシェルだけが終了する。シェルタブは workspace ごとに永続化される。
- Files の変更バッジと Changes の一覧が同じ `git status` を示す。
- Changes の行から Files のプレビューにファイルを開き、View と Diff を切り替えられる（Edit は Phase 2）。
- worktree A のセッションからメイン repo / worktree B のファイルを開こうとすると拒否される。
- ハブ経由では Files が無効になり、理由とローカル接続の案内が出る。
- ノードがオフラインでも、既に開いているプレビューの内容は読み取り専用で保持される。保存は失敗し、復帰後に再試行できる。
- ファイル保存後、Changes の件数とバッジが更新される。
- デスクトップ / モバイルをまたいでも PTY が二重に起動しない。

## 9. リスクと代替

- **ファイルタブの複雑さ**: 外側ツールは固定にし、可変タブは Files 内に限定した。ツリーとプレビューの状態が増えるため、store のテストで永続化・上限・不正エントリを固める。
- **workspace ごとの状態増加**: localStorage のサイズと古いエントリ。直近数件に制限し、読み込み時に不正・削除済みを捨てる。
- **非表示ターミナルのリソース**: ドックを開いている間は WS を維持する。ドックを閉じたら detach して TTL に戻す。外側ツール切替のたびに再接続する代替は、replay のちらつきと入力遅延が出る。
- **Monaco のメモリ**: File プレビューごとにモデルを持つ。閉じたら dispose し、ファイルタブ上限を守る。モバイルは Shiki で回避する。
- **パス検証の穴**: Git toplevel を境界にすると、サブディレクトリセッションが repo 全体に触れる。登録済み workspace から解決した root だけを許可し、worktree / メイン repo の分離をテストする。
- **シンボリックリンク**: 外へ出るリンクを拒否すると、モノレポの一部が開けないことがある。セキュリティを優先し、必要ならリンク先が root 内のものだけ許可する。
- **大きなディレクトリ**: `node_modules` などの一覧で UI が固まる。ignore と件数上限で防ぐ。必要なら `ignore` クレートを追加する。
- **リモート編集の危険**: 任意ファイルの書き換えはシェルに近い権限。Phase 1 はローカルのみとし、解禁時はノード設定と監査を必須にする。
- **代替案 (却下)**: プロジェクト単位の 1 ドック。worktree とメイン repo の編集先が曖昧になり、誤編集の危険がある。
- **代替案 (却下)**: Changes の中だけでファイルを開く。タブが増えず単純だが、複数ファイルの比較と編集の行き来が苦しい。
- **代替案 (却下)**: OS の外部エディタを開く。リモートノードとモバイルで成立しない。

## 10. 未決事項

- `ignore` クレートを追加するか、`git check-ignore` をディレクトリ単位で呼ぶか。
- モバイルでの編集を将来的に許可するか。
- ハブ経由の読み取りを解禁する際の設定名と既定値（`allow_remote_fs` / read-only / read-write）。
- ファイルタブの並べ替え・ピン留めを Phase 3 に入れるか。

## 11. 実装状況

### Phase 0 (2026-11-14 実装済み)

- `ui/src/lib/stores/dock.svelte.ts`: workspace キーのアクティブツール / Files 状態、
  永続化、ファイルタブ上限、workspace 上限、壊れたエントリの破棄。
- `ui/src/lib/dock/registry.ts`: Files / Changes / Terminal の固定ツール定義。
- `ui/src/lib/components/dock/`: `DockHost`（固定ツールのヘッダー + 本体）/
  `DockRail` / `ChangesTool` / `TerminalTool`。外側の可変タブバーとツールシートは
  持たない。
- モバイルは `SessionHeader` に Files / Changes / Terminal のランチャーアイコンを
  置き、1 タップで開けるようにした。
- セッション画面から `activePanel` と幅のローカル state を削除し、旧
  `SessionPanelHost` / `SessionToolRail` / `SessionToolsSheet` / `session-panels.ts`
  を削除した。
- `TerminalView` に `active` を追加し、非アクティブ時は hidden でマウントを維持、
  再アクティブ時に fit / resize、ドックを閉じたときだけ detach する。
- Terminal ツールに複数シェルタブを追加した。PTY は workspace + `terminal_id` で
  一意になり、`+` で追加、× / 中クリックで kill する。詳細は
  [`2026-11-14-terminal-tabs.md`](../changelog/2026-11/2026-11-14-terminal-tabs.md)。
- `docs/07` §3.2 / §4 と changelog に Phase 0 の確定仕様を反映した。

> 当初は外側も可変タブ（Files / Changes / Terminal / File）にしていたが、
> ファイルが増えるとツール切替が埋もれるため、外側を固定し、可変ファイルタブを
> Files の中へ移すハイブリッドに変更した（2026-11-14）。

### Phase 1 (2026-11-14 実装済み)

- `fxg-protocol`: workspace fs / git status / git diff の型と `NodeRpc` の
  request / response を追加。`GET /api/v1/nodes/:id/workspace/{fs,git}/*` を
  ハブ / ノードの共通ルーターに追加した。
- `fxg-node`: `workspace` モジュールを追加。登録済み workspace から access root
  （worktree toplevel）を解決し、`.git` とシンボリックリンク脱出を拒否する。
  ディレクトリ一覧、UTF-8 テキスト（2 MiB 上限）、バイナリ / 超過の列挙、
  `git status --porcelain=v2 -z`、単一ファイル diff、未追跡ファイルの
  合成 diff を実装した。ハブ経由の workspace RPC は `FORBIDDEN` にする。
- `ui`: Files ツリー + プレビュー、git status バッジ、変更のみ一覧、ファイルタブ
  （Shiki の View / 単一ファイル Diff / Markdown Preview / Source / 画像）、Changes の
  Session / Worktree / Branch 一覧を追加した。Changes / チャットからは Files の
  プレビューにファイルを開く。ハブ接続時は Files を無効化し、Changes は従来の
  インライン Diff にフォールバックする。
- フォローアップ (2026-11-14): ツリー / プレビューの分割比をドラッグで変更し、
  workspace ごとに保持するようにした。PNG / JPEG / GIF / WebP / BMP / AVIF / ICO を
  data URL で画像プレビューする。
- テスト: status パーサー、`.git` 拒否、ディレクトリ一覧、未追跡 diff、
  workspace 境界（登録外パス / サブディレクトリ / シンボリックリンク脱出）を
  Rust で検証した。

**Phase 1 の既知の制約**: 編集は未対応。SVG は画像ではなくテキストとして表示する。
`.gitignore` の完全な解釈はせず、UI が既定の重いディレクトリを隠す。ワークスペース
監視による自動更新は未対応（手動更新 / タブ表示時 / ターン完了時）。

Phase 2 以降（エディタ・保存・競合検出）は未着手。
実装が進んだ時点で、確定した仕様を `docs/07` / `docs/03` / `docs/01` に昇格し、
この Proposal のステータスを更新する。
