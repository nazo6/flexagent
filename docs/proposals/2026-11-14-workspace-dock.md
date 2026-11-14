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

- セッション画面の右ペインを、**workspace スコープのタブ式ツールドック**にする。
  - **workspace**: `(node_id, canonical local_path)`。ターミナルと同じ同一性。
  - **レール**: ツールの起動器。パネル選択の状態は持たない。
  - **タブ**: ツールのインスタンス。同一性は `tool + resource`（`File` はパス、`Changes` / `Terminal` は singleton）。
- ドックのタブ構成・アクティブタブ・幅・開閉は **workspace ごと**に保持し、セッション切替では閉じない。
  - 現在の `activePanel = null` リセットを廃止する。
- ツールは **Files / Changes / File / Terminal**。Files と Changes は `git status` を共有し、変更行から File タブを開く。File タブ内で `Diff | Edit` を切り替える。
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

### 2.2 タブモデル

- タブは **ツール種別** と **リソース** の組で識別する。
  - `Changes`: singleton（workspace に 1 つ）。
  - `Terminal`: singleton（workspace に 1 つ）。
  - `Files`: singleton のエクスプローラ。ファイルを開く操作だけを行う。
  - `File`: リソースはパス。同じパスのタブは再利用する。
- ツールはレジストリに定義する。追加はレジストリと描画分岐で完結させ、`SessionPanelId` のような排他的な union を UI 全体に配らない。
- ツールの定義は次を持つ（型の正は実装）。
  - `id` / `label` / `icon`
  - レールに出すか（`Files` / `Changes` / `Terminal` を出し、`File` は出さない）
  - singleton か、リソース必須か
  - 非アクティブでもマウントを維持するか（ターミナル用）
- タブは開いた順に並べる。ピン留めや並べ替えは Phase 3 以降。
- 上限は 8。超えたら最も古い非アクティブタブを閉じる。singleton は常に残す。

### 2.3 状態と永続化

- ドックの状態はアプリレベルの store に置く。
  - workspace キーごとの `tabs`（tool / resource / mode）、`activeTabId`、`collapsed`。
  - ユーザー共通の `width`（既存の割合ベースを `clamp(22rem, N%, 75%)` で使う）。
- `localStorage` に保存するのはタブのメタデータと幅だけ。ファイル内容・未保存バッファ・スクロール位置は保存しない。
  - リロード後、File タブは再読込し、Terminal タブは attach + replay で復元する。
- 保存対象は直近の workspace 数件に制限し、削除済みワークスペースのエントリは読み込み時に捨てる。
- workspace が変わったときだけ、保存 → 対象 workspace の状態を読み込む。同じ workspace のセッション移動では何もしない。
- Changes の選択ファイルやフィルタはセッション画面の store に持たせ、タブ切替で失わない。workspace をまたぐとリセットする。
- 初回の workspace は畳んだ状態から始める。右ペインを勝手に開かない。

### 2.4 ライフサイクル

- **ドックを開く**: レールのアイコンでツールを開く、または既存タブをアクティブにする。
- **タブを閉じる**: 最後のタブを閉じたらドックを畳む。ターミナルのタブを閉じた場合は unmount して detach する（PTY は残る）。
- **セッション切替**: 同じ workspace ならタブと PTY を維持する。別 workspace なら旧ドックを unmount（ターミナルは detach）し、新 workspace の状態を読み込む。
- **ドックを畳む**: ターミナルは unmount → detach。再度開くと `open` が冪等に attach + replay する。
- **ターミナルのタブ切替**: ドックが開いている間はターミナルを hidden でマウントしたままにし、WS を維持する。再アクティブ時に `fit()` と resize を送る。
  - 非表示中に 0 サイズで fit しない。active 復帰時に再 fit する。
  - 他のツールは active なタブだけをマウントする。File の未保存バッファは store に持たせる。
- **worktree 削除**: 該当 workspace のタブを閉じる。PTY の kill はターミナル側の worktree 連動に従う。
- **ノード切断**: タブは残し、内容を読み取り専用にする。保存は失敗させ、復帰後に再試行させる。

### 2.5 UI 構成

- **レール（右端）**: ツールのアイコン。バッジ（Changes の件数、Terminal の実行中など）を出す。最下部にレール自体を隠すボタン。ドックの開閉はタブバー右端のボタンで行う。
- **タブバー**: アイコン + 短いラベル + 未保存ドット + 閉じる。File タブは basename を表示し、tooltip にフルパス。オーバーフローは横スクロール。
- **ツールバー**: タブの下にツール固有の操作を置く。
  - Files: パンくず、更新、隠しファイル / 変更のみの切替。
  - Changes: スコープ（Session / Worktree / Branch）、更新、フィルタ。
  - File: パンくず、`Diff | Edit`、保存、競合表示。
  - Terminal: 接続状態、再接続、新しいシェル。
- **デスクトップ**: チャットとドックの分割。既存のリサイズハンドルを維持する。ドックは `hidden lg:flex`。
- **モバイル**: セッションヘッダーの Changes / Terminal アイコンから**1 タップ**で開く。会話の上に**背景を暗くしたオーバーレイ**としてタブバー付きのドックを出し、オーバーレイの外側（背景）のタップ、または右上の閉じるボタンで畳む。会話は背後に残すが、`inert` にして操作・フォーカスの対象から外す。ツールが増えた場合は、ドック内の `+` からツールシートを開く。デスクトップとモバイルのスロットはビューポートで切り替え、片方だけをマウントする。
- **タブのアクセシビリティ**: `role="tablist"` / `role="tab"` / `aria-selected` とし、左右キーで移動できるようにする。
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
- 一覧の +/- は `git diff --numstat` から取る。worktree 全体の unified diff は一覧のためには取得しない。
- ファイルを選択したときに初めて単一ファイルの diff を取得する。大きなリポジトリでも一覧が軽い。
- 行をクリックすると File タブを Diff モードで開く。Diff は File タブのツールバーで切り替えられる。
- ファイルがディスク上に存在しない（削除済み）場合は、diff のみの読み取り専用タブとして開く。

### 3.3 File

- 1 タブ = 1 パス。タブ内で `Diff | Edit` を切り替える。
- **View**: デスクトップは Monaco の読み取り専用、モバイルは Shiki。大きすぎるファイル・バイナリは案内表示。
- **Diff**: 単一ファイルの unified diff。既存の `DiffViewer` の表示規則を踏襲する。
- **Edit**（Phase 2）:
  - Monaco の通常エディタ。言語は拡張子から解決する（既存の `MonacoDiff` と同じ対応表を使う）。
  - 未保存はタブにドットを出す。`Cmd/Ctrl+S` で保存。
  - 保存は楽観ロック。読み込み時のハッシュが変わっていたら `CONFLICT` とし、上書き・再読込を選ばせる。
  - 外部変更を検知したらバナーで知らせ、保存時に必ず確認する。
  - モバイルの編集は Phase 2 では無効（閲覧のみ）とし、必要になったら対応する。
- 行ジャンプ（チャットの差分から開く）は Phase 3。

### 3.4 Terminal

- workspace に 1 つ。既存の `TerminalView` をツールとして包む。
- ツールバーに接続状態・再接続・新しいシェルを置く。既存のステータスバッジと仮想キーバーは維持する。
- ドックが開いている間は hidden でもソケットを維持し、タブ切替で PTY を detach しない。ドックを閉じたときだけ detach する。
- 再接続は既存の `open`（冪等）と replay を使う。新しいシェルは明示操作のときだけ。

### 3.5 将来のツール

レジストリに追加するだけで、レール・モバイルシート・タブバーに現れる形にする。

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
- `DockHost` / `DockTabBar` / `DockRail` / `DockToolsSheet` を追加する。
- `session-panels.ts` / `SessionPanelHost.svelte` / `SessionToolRail.svelte` / `SessionToolsSheet.svelte` を削除し、ドックへ一本化する（ラッパーを残さない）。
- セッション画面の `activePanel` と幅のローカル state を削除し、ドック store に移す。
- 既存の Changes / Terminal をツールとして移植する。
- Terminal に active 制御を追加し、非表示中の fit を避ける。ドックを閉じたときだけ detach する。
- モバイルのツールシートと全画面ドックを維持する。片肺マウントの不変条件を守る。
- Vitest で store を検証する（workspace 切替、singleton の重複、永続化、タブ上限、復元時の不正エントリ）。
- 受け入れ: 同じ workspace のセッション移動で Terminal が閉じない / リロードでタブが戻る / 別 workspace でタブセットが切り替わる / 最後のタブでドックが畳まれる。

### Phase 1: 閲覧と連携

- ノード / ハブに workspace fs / git status / 単一ファイル diff API を追加する（ローカル接続のみ）。
- Files ツリーと git status の装飾、変更のみフィルタを実装する。
- Changes を git status + セッション diff の統合一覧にし、単一ファイル diff を遅延取得する。
- File タブ（View / Diff）と、Changes / Files / チャットからの導線を実装する。
- 削除済みファイル・バイナリ・大きすぎるファイル・ノードオフラインの表示を整える。
- 受け入れ: Files から開いたファイルが Changes と同じ変更状態を示す / worktree の外に出られない / ハブ経由では明示的に無効。

### Phase 2: 編集

- 保存 API、楽観ロック、競合 UI、監査を追加する。
- Monaco の編集タブ、未保存ドット、`Cmd/Ctrl+S`、外部変更バナーを実装する。
- モバイルは閲覧のみとし、編集はデスクトップに限定する。
- 受け入れ: 保存後に Changes の状態が更新される / 競合時に上書き・再読込を選べる / 権限の無い接続では保存できない。

### Phase 3: 仕上げ

- ノードのファイル監視と WS 通知（debounce）を追加する。
- チャットの差分から File タブの該当行へジャンプする。
- 画像ファイルのプレビュー、タブの並べ替え・ピン留め、workspace search などの追加ツール。
- プロジェクト画面から worktree ごとのドックを開く導線。
- ターミナルのシェル統合（cwd 追跡）と「ここでターミナルを開く」。

## 7. 不変条件

1. ドックの状態キーは `(node_id, canonical local_path)`。プロジェクトやセッションをキーにしない。
2. セッション切替はドックの寿命を決めない。workspace が変わったときだけタブセットを入れ替える。
3. 同じ workspace のターミナルは 1 本。既存の workspace スコープと参照カウント / TTL をそのまま使う。
4. ターミナルはドックが開いている間だけ hidden 保持し、ドックを閉じたら detach する。ソケット切断では kill しない。
5. デスクトップとモバイルのスロットは片方だけをマウントする。1 つの `TerminalView` は最大 1 つの PTY に接続する。
6. ファイル API は登録済み workspace の access root 配下だけを扱う。worktree / メイン repo / 別 worktree をまたがない。
7. Phase 1 のファイル API はローカル接続のみ。ハブ経由は明示的に拒否する。
8. 書き込みは既存ファイルに限定し、期待ハッシュが一致したときだけ成功する。作成・削除・リネームは今回の範囲外。
9. タブのメタデータだけを localStorage に保存する。内容・未保存バッファは保存しない。
10. ツールの追加はレジストリと描画分岐で行い、排他的な `SessionPanelId` を復活させない。

## 8. 受け入れ条件（全体）

- 同じ workspace の別セッションへ移動しても、開いていたタブとターミナルのシェルが残る。
- 別 workspace へ移動すると、その workspace のタブセットに切り替わる。ターミナルもその worktree の PTY になる。
- リロード後、タブ構成が復元され、Terminal は attach + replay で scrollback が戻る。
- Files の変更バッジと Changes の一覧が同じ `git status` を示す。
- Changes の行から File タブを開き、Diff と Edit を切り替えられる。
- worktree A のセッションからメイン repo / worktree B のファイルを開こうとすると拒否される。
- ハブ経由では Files が無効になり、理由とローカル接続の案内が出る。
- ノードがオフラインでも、既に開いているタブの内容は読み取り専用で保持される。保存は失敗し、復帰後に再試行できる。
- ファイル保存後、Changes の件数とバッジが更新される。
- デスクトップ / モバイルをまたいでも PTY が二重に起動しない。

## 9. リスクと代替

- **タブの複雑さ**: 状態が増える。Phase 0 で store とテストを先に固め、ツールは 2 つから始める。代替として単一パネル + クイック切替があるが、ファイルと Changes を行き来する主要動線で現在位置を失う。
- **workspace ごとの状態増加**: localStorage のサイズと古いエントリ。直近数件に制限し、読み込み時に不正・削除済みを捨てる。
- **非表示ターミナルのリソース**: ドックを開いている間は WS を維持する。ドックを閉じたら detach して TTL に戻す。タブ切替のたびに再接続する代替は、replay のちらつきと入力遅延が出る。
- **Monaco のメモリ**: File タブごとにモデルを持つ。閉じたら dispose し、タブ上限を守る。モバイルは Shiki で回避する。
- **パス検証の穴**: Git toplevel を境界にすると、サブディレクトリセッションが repo 全体に触れる。登録済み workspace から解決した root だけを許可し、worktree / メイン repo の分離をテストする。
- **シンボリックリンク**: 外へ出るリンクを拒否すると、モノレポの一部が開けないことがある。セキュリティを優先し、必要ならリンク先が root 内のものだけ許可する。
- **大きなディレクトリ**: `node_modules` などの一覧で UI が固まる。ignore と件数上限で防ぐ。必要なら `ignore` クレートを追加する。
- **リモート編集の危険**: 任意ファイルの書き換えはシェルに近い権限。Phase 1 はローカルのみとし、解禁時はノード設定と監査を必須にする。
- **代替案 (却下)**: プロジェクト単位の 1 ドック。worktree とメイン repo の編集先が曖昧になり、誤編集の危険がある。
- **代替案 (却下)**: Changes の中だけでファイルを開く。タブが増えず単純だが、複数ファイルの比較と編集の行き来が苦しい。
- **代替案 (却下)**: OS の外部エディタを開く。リモートノードとモバイルで成立しない。

## 10. 未決事項

- Git toplevel をファイル境界にする際、workspace の登録判定を `session.local_path` まで含めるか、binding のみにするか（実装時に既存の登録経路を確認する）。
- `ignore` クレートを追加するか、`git check-ignore` をディレクトリ単位で呼ぶか。
- File タブの言語解決を MonacoDiff の対応表と共有するか、別モジュールに切り出すか。
- モバイルでの編集を将来的に許可するか。
- ハブ経由の読み取りを解禁する際の設定名と既定値（`allow_remote_fs` / read-only / read-write）。
- タブの並べ替え・ピン留めを Phase 3 に入れるか。

## 11. 実装状況

### Phase 0 (2026-11-14 実装済み)

- `ui/src/lib/stores/dock.svelte.ts`: workspace キーのタブ状態、永続化、タブ上限、
  workspace 上限、壊れたエントリの破棄。
- `ui/src/lib/dock/registry.ts`: Changes / Terminal のツール定義。
- `ui/src/lib/components/dock/`: `DockHost` / `DockTabBar` / `DockRail` /
  `DockToolsSheet` / `ChangesTool` / `TerminalTool`。
- モバイルは `SessionHeader` に Changes / Terminal のランチャーアイコンを置き、
  1 タップで開けるようにした。ツールシートはドック内の `+` から開く。
- セッション画面から `activePanel` と幅のローカル state を削除し、旧
  `SessionPanelHost` / `SessionToolRail` / `SessionToolsSheet` / `session-panels.ts`
  を削除した。
- `TerminalView` に `active` を追加し、非アクティブ時は hidden でマウントを維持、
  再アクティブ時に fit / resize、ドックを閉じたときだけ detach する。
- `docs/07` §3.2 / §4 と changelog に Phase 0 の確定仕様を反映した。

Phase 1 以降（Files / git status / 単一ファイル diff / エディタ）は未着手。
実装が進んだ時点で、確定した仕様を `docs/07` / `docs/03` / `docs/01` に昇格し、
この Proposal のステータスを更新する。
