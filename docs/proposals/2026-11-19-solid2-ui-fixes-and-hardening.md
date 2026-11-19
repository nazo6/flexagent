# 2026-11-19 Solid 2 Web UI の不具合修正・リアクティビティ健全化計画

- **日付**: 2026-11-19
- **ステータス**: `Implemented`
- **対象パッケージ**: `ui`（補助的にルート設定・`docs/`）
- **対象スクリプト / 設定**: `mise run check:ui` / `mise run test:ui` / `mise run lint:ui` / `mise run build:ui` / `mise run fmt`
- **関連ドキュメント**:
  - [`docs/proposals/2026-11-18-solid2-ui-migration.md`](./2026-11-18-solid2-ui-migration.md)（Solid 2 移行計画）
  - [`docs/changelog/2026-11/2026-11-18-solid-semantics-pass.md`](../changelog/2026-11/2026-11-18-solid-semantics-pass.md)（初回セマンティクス整理）
  - [`ui/CONVENTIONS.md`](../../ui/CONVENTIONS.md)（UI 実装規約）
  - [`docs/07-web-ui-and-pwa.md`](../07-web-ui-and-pwa.md)（Desk UI 仕様）
  - [`docs/backlog.md`](../backlog.md)（残タスク一覧）

---

## 0. 背景と目的

`ui-solid` ブランチで Svelte 5 / SvelteKit から Solid 2 + Terracotta 2 + `@solidjs/router` 2 への移行が完了し、`tsc` / Vitest / `oxfmt` は通過する状態になっている。しかし詳細レビューの結果、**Solid 2 特有のリアクティビティ仕様**・**Terracotta ラッパー実装**・**Svelte 時代のイディオム残存**に起因する機能破壊、リグレッション、ライフサイクル不具合、およびコードスメルが複数箇所で残っていることが判明した。

本 Proposal は、レビューで検出されたすべての不具合と改善項目を漏れなく整理し、段階的な修正方針と完了条件を定める。

### 根本原因の 4 つの分類

1. **Solid 2 の書き込み遅延フラッシュ**: `createSignal` の setter はマイクロタスクのフラッシュまで反映されないため、同一ターン内の `setX(v); x()` は更新前の値を返す（`await` 後の DOM 計測も `flush()` 前は未反映）。
2. **`createEffect(compute, apply)` の戻り値非比較**: Solid 2 の `createEffect` は `compute` が再実行されると戻り値が同値でも cleanup + `apply` を再実行する。参照が毎回変わるオブジェクトを `compute` で返すと、不要な再フェッチ・ツリー折りたたみ・タイマー再生成が連鎖する。
3. **`<For>` のデフォルト参照比較**: `keyed` 省略時は参照同一性で照合されるため、API/WS 更新で配列要素のオブジェクトが差し替わると行コンポーネントが破棄・再マウントされ、ダイアログや入力状態が失われる。
4. **`apply` 内・コンポーネント本体での非トラッキング読み取りと Svelte 流の持ち越し**: `apply` コールバック内での `strictRead` 違反 (`[STRICT_READ_UNTRACKED]`)、`let` 変数への DOM `ref` 保存、`ownedWrite: true` による所有スコープ保護の無効化、および非制御対応・フォーカス・スクロールロックの不足した UI プリミティブ群。

---

## 1. 修正項目・改善項目の詳細

### 1.1 Solid 2 遅延フラッシュによる Write-then-Read バグ（Critical）

| 対象                                                                                                                                                                          | 症状と原因                                                                                                                                                                                                                                                                                                                                                                                                                                        | 修正方針                                                                                                                                                          |
| :---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | :---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`ExternalSessionsDialog.tsx`](../../ui/src/lib/components/session/ExternalSessionsDialog.tsx#L67-L110)                                                                       | ダイアログオープン時に `setSelectedNodeId(pickNode()); void load(true);` を呼ぶが、`load()` 内の `selectedNodeId()` が未フラッシュで `""` のまま。さらに `load()` 内でも `setSelectedNodeId` 直後に `selectedNodeId() === ""` を判定するため、Hub モードでは常に `error_node_offline` で失敗する。`<select>` 切替時（L180–185）も変更前のノードを取得する。加えて `createEffect` `apply` 内で `pickNode()` / `load()` が store を生読みしている。 | `load(force, nodeId)` のように対象 `nodeId` を引数で直接受け渡す。`createEffect` の `compute` でオープン状態と既定ノードを計算し、`apply` へ渡す。                |
| [`NewSessionChat.tsx`](../../ui/src/lib/components/session/NewSessionChat.tsx#L493-L599)                                                                                      | ① `applyCapabilities` が `setCapabilities` / `setSelectedConfigValues` 直後に `currentModelConfig()` を呼ぶため、未フラッシュの旧値を見て依存オプション（`effort` 等）の追加 probe が走らない。② `probeKey` effect が `setCapabilities(null)` / `setSelectedConfigValues({})` 直後に `currentModelConfig()` を呼ぶため、エージェント切替時に切替前エージェントのモデル ID で新エージェントを probe する。                                         | `currentModelConfig` が `(preview, selectedValues)` を引数に受け取れるよう純関数化し、effect 切替時は空（または対象エージェント用の保存済み設定）を明示的に渡す。 |
| [`NewSessionChat.tsx`](../../ui/src/lib/components/session/NewSessionChat.tsx#L855-L869) / [`L1523-1526`](../../ui/src/lib/components/session/NewSessionChat.tsx#L1523-L1526) | `selectAgent`, `onManualAgentInput`, `ModePicker` `onchange` が `setChosenAgentId` / `setChosenMode` 直後に `persistPrefs()` を呼んでおり、フラッシュ前の値が `localStorage` に保存される（1 ステップ遅れる）。`toggleManualAgent`（L872–876）も未フラッシュ状態に暗黙依存している。                                                                                                                                                              | `persistPrefs({ agent, mode })` のように確定値を引数で渡す。`toggleManualAgent` もローカル変数 `const next = !manualAgent()` を計算してから両 setter を呼ぶ。     |
| [`SessionPage.tsx`](../../ui/src/routes/SessionPage.tsx#L356-L375)                                                                                                            | `loadOlder` が `await sync.loadOlderSessionEvents(sessionId())` の直後に `container.scrollHeight` を計測しているが、Solid 2 のバッチフラッシュ前なので DOM 高さが未更新となりスクロール位置が維持されない。                                                                                                                                                                                                                                       | `scrollHeight` 計測直前に `solid-js` の `flush()` を呼ぶ。                                                                                                        |

---

### 1.2 `createEffect` の戻り値非比較による不要リロード・破壊カスケード（Critical）

`sync.refreshSessions()` 等のたびに `session()` や `dockContext()` が新しいオブジェクト参照を生成するため、プリミティブ値の `createMemo` を挟んでいないエフェクトが WS 更新ごとに再実行されている。

| 対象                                                                                 | 症状                                                                                                                                                                                                                                                                                   | 修正方針                                                                                                                                              |
| :----------------------------------------------------------------------------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | :---------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`FilesTool.tsx`](../../ui/src/lib/components/dock/FilesTool.tsx#L131-L136)          | `dockContext` の参照変化ごとに `reload()` が走り、`expanded` が初期化されて展開中のファイルツリーがセッション更新のたびに閉じる。                                                                                                                                                      | `createMemo(() => \`${props.ctx.nodeId}\0${props.ctx.localPath}\0${refreshToken()}\`)` 等の単一プリミティブキーを監視する。                           |
| [`FilePreview.tsx`](../../ui/src/lib/components/dock/FilePreview.tsx#L82-L98)        | ピン留め・行番号更新・他タブ操作による `props.tab` 参照変化で同一ファイルでも再フェッチし `CodeView`/`DiffViewer` を再生成する。また `props.ctx.nodeId` / `props.ctx.localPath` / `remote()` が `compute` に無くワークスペース切替に追従しない（`apply` 内の untracked read も発生）。 | `nodeId`, `localPath`, `tab.path`, `tab.mode`, `tab.scope`, `refreshToken` を含むプリミティブキーを `createMemo` で作り、`apply` は引数から値を読む。 |
| [`AgentUsage.tsx`](../../ui/src/lib/components/session/AgentUsage.tsx#L105-L119)     | `props.session` 参照変化ごとに 60 秒ポーリングタイマーを破棄・再生成し `/usage` API を即座に叩く。また cleanup で `requestId` を進めないためアンマウント後に `statusTimer` がリークしうる。`node`/`agent` 変更時に `usage` / `hasLoaded` もリセットされない。                          | キー文字列の `createMemo` 経由で購読し、切替・cleanup 時に `++requestId` と状態リセットを行う。                                                       |
| [`ThoughtBlock.tsx`](../../ui/src/lib/components/chat/ThoughtBlock.tsx#L28-L41)      | `const streaming = () => props.item.streaming;` が `createMemo` ではないため、思考ストリームのトークン 1 個ごとにタイマー再作成と `setNow(Date.now())` が発生する。                                                                                                                    | `streaming` を `createMemo(() => props.item.streaming)` に変更する。                                                                                  |
| [`SessionHeader.tsx`](../../ui/src/lib/components/session/SessionHeader.tsx#L73-L82) | 未メモ化オブジェクト `{ nodeId, agentId }` によりセッション更新ごとにエージェントアイコンを取り直す。                                                                                                                                                                                  | キー文字列の `createMemo` に変更する。                                                                                                                |
| [`SessionPage.tsx`](../../ui/src/routes/SessionPage.tsx#L242-L265)                   | `listed: sync.sessions.some(...)` が未メモ化のため、未収載のディープリンク表示中に `sync.sessions` が更新されるたび `sessionDetail` をキャンセル＆再取得する。                                                                                                                         | `listed` を独立した `createMemo` に切り出す。                                                                                                         |
| [`MonacoDiff.tsx`](../../ui/src/lib/components/diff/MonacoDiff.tsx#L16-L75)          | 未メモ化 `{ element, path, oldText, newText }` により `props.file` 参照が変わると同内容でも Monaco Diff エディタ + Worker を破棄・再生成する。                                                                                                                                         | `createMemo`（`equals` またはキー化）で同一内容時の再初期化を防ぐ。                                                                                   |

---

### 1.3 `<For>` の `keyed` 欠落・不適切なキー指定（Critical）

配列要素のオブジェクトが更新のたびに新規生成される箇所で `keyed` が抜けているため、子コンポーネントが保持するダイアログ・ポップオーバー・スクロール・入力状態が消失している。逆に、重複しうる文字列配列には `keyed={false}` を明示する必要がある。

| 対象                                                                                                                                                                                                                                                                                | 現状と問題                                                                                                                                                                                                                                                         | 修正方針                                                                                                                                              |
| :---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | :----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | :---------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`AppSidebar.tsx`](../../ui/src/lib/components/layout/AppSidebar.tsx#L309) / [`L333`](../../ui/src/lib/components/layout/AppSidebar.tsx#L333) / [`L391`](../../ui/src/lib/components/layout/AppSidebar.tsx#L391) / [`L460`](../../ui/src/lib/components/layout/AppSidebar.tsx#L460) | `projectGroups()`・`group.sessions`・`needsAttentionSessions()`・`archivedSessions()` すべて `keyed` 無し。WS 更新で `SessionRow` が再マウントされ、開いている `SessionActionsMenu`（Dropdown / Rename Dialog / Delete AlertDialog）やツールチップが即座に閉じる。 | `keyed={(g) => g.key}` / `keyed={(s) => s.session_id}` を付与し、コールバック引数 `group()` / `session()` を JSX・memo 内からアクセスする形へ改める。 |
| [`ProjectsPage.tsx`](../../ui/src/routes/ProjectsPage.tsx#L132)                                                                                                                                                                                                                     | `sync.projects` の `<For>` に `keyed` が無く、`refreshProjects()` で `ProjectCard` の Unlink 確認ダイアログが閉じる。                                                                                                                                              | `keyed={(p) => p.project_id}` を付与する。                                                                                                            |
| [`Composer.tsx`](../../ui/src/lib/components/chat/Composer.tsx#L448) / [`NewSessionChat.tsx`](../../ui/src/lib/components/session/NewSessionChat.tsx#L1566)                                                                                                                         | `configOptions()` の `<For>` に `keyed` が無く、probe 更新で `ConfigOptionPicker`（開いているポップオーバー・検索文字列・optimistic 値）が破棄される。                                                                                                             | `keyed={(o) => o.key}` を付与する。                                                                                                                   |
| [`ConfigOptionPicker.tsx`](../../ui/src/lib/components/composer/ConfigOptionPicker.tsx#L189) / [`L197`](../../ui/src/lib/components/composer/ConfigOptionPicker.tsx#L197)                                                                                                           | `favoriteChoices()` / `otherChoices()` に `keyed` が無い。                                                                                                                                                                                                         | `keyed={(c) => c.value}` を付与し、後述の `choiceRow` の untracked read も合わせて解消する。                                                          |
| [`ToolCallGroup.tsx`](../../ui/src/lib/components/chat/ToolCallGroup.tsx#L100) / [`L326`](../../ui/src/lib/components/chat/ToolCallGroup.tsx#L326)                                                                                                                                  | L100 の `props.tools` に `keyed` が無く、ツール進捗ごとに DOM・`<pre>` スクロール・選択状態が失われる。L326 の `tool.locations`（文字列配列）は重複時に誤動作しうる。                                                                                              | L100 に `keyed={(t) => t.toolCallId}`（コールバック内 L102–116 を関数/memo 化）、L326 に `keyed={false}` を指定する。                                 |
| [`AuditLog.tsx`](../../ui/src/lib/components/settings/AuditLog.tsx#L97)                                                                                                                                                                                                             | `logs()` に `keyed` が無く、リフレッシュで開いていた `<details>` が閉じる。                                                                                                                                                                                        | `keyed={(entry) => entry.id}`（または一意キー）を付与する。                                                                                           |
| [`DiffText.tsx`](../../ui/src/lib/components/diff/DiffText.tsx#L33)                                                                                                                                                                                                                 | `string[]` に対し参照同一性キーのため、空行や `}` 等の同一文字列行でリスト更新時に再配置・再利用の不整合が起きうる。                                                                                                                                               | `<For each={lines()} keyed={false}>` に変更する。                                                                                                     |

---

### 1.4 `STRICT_READ_UNTRACKED`・非リアクティブ DOM ref・自己書き込み Effect（Critical〜Major）

1. **`ChatTimeline.tsx` の行コンテキスト固定化（Vitest stderr で発生中）**:
   - [`ChatTimeline.tsx`](../../ui/src/lib/components/chat/ChatTimeline.tsx#L161): `SingleRow` 本体で `const ctx = props.ctx;` としているため、行マウント後の `revert` / `lastForkableSeq` / `sessionStopped` 変化が行に届かず、`STRICT_READ_UNTRACKED` を出している。`props.ctx.*` を直接参照するかアクセサ経由で読む。
2. **非リアクティブ `let` DOM ref によるスクロール不全**:
   - [`CodeView.tsx`](../../ui/src/lib/components/dock/CodeView.tsx#L15) / [`L43-57`](../../ui/src/lib/components/dock/CodeView.tsx#L43-L57): `<Show when={html() !== null}>` 内の `<div ref>` が plain `let scroller` を書き換えるため、初回ハイライト時の effect が `undefined` を掴んで行指定ジャンプ（`#L123`）に失敗する。`createSignal<HTMLDivElement | null>(null)` へ置き換える。
   - [`ProvisioningLogCard.tsx`](../../ui/src/lib/components/ProvisioningLogCard.tsx#L9-L18): 同様に `scrollEl` を `createSignal` 化し、折りたたみ時にデタッチ済み `<pre>` を指し続けないよう制御する。また L14 で `props.lines.length` を effect `apply` 内で直接読んでいる（`compute` 戻り値を使う）。
3. **`ChangesTool` のワークスペース追従漏れ**:
   - [`ChangesTool.tsx`](../../ui/src/lib/components/dock/ChangesTool.tsx#L96-L109): 2 つの `createEffect` の `compute` が `props.ctx.nodeId` / `props.ctx.localPath` を読まず `apply` 内で読んでいるため、別ワークスペースのセッションへ切り替えても前ワークスペースの git status / diff が残り続ける（`STRICT_READ_UNTRACKED` も発生）。
4. **`SessionPage` の自己書き込み Effect とリスナーリーク・状態未リセット**:
   - [`SessionPage.tsx`](../../ui/src/routes/SessionPage.tsx#L287-L306): `compute` で `followOutput()` を購読し `apply` で `setFollowOutput(true)` を書くため `EFFECT_WRITES_OWN_SOURCE` が起きる。`compute` から `followOutput` を外し、必要なら `untrack(followOutput)` または別エフェクトに分離する。
   - [`SessionPage.tsx`](../../ui/src/routes/SessionPage.tsx#L184-L208): `startResize` が登録する `window` の `pointermove` / `pointerup` にアンマウント時 cleanup と `pointercancel` 処理がない。
   - [`SessionPage.tsx`](../../ui/src/routes/SessionPage.tsx#L159-L165): `sessionId` 切替時に `resumeRequired`, `followOutput`, `showScrollBottom` をリセットする。
5. **その他の `STRICT_READ_UNTRACKED` と非同期レース**:
   - [`ConfigOptionPicker.tsx`](../../ui/src/lib/components/composer/ConfigOptionPicker.tsx#L111): `<For>` コールバック直下で `isFavoriteChoice(choice)` を呼んでいる → JSX / accessor 内で読む。
   - [`ComposerTextarea.tsx`](../../ui/src/lib/components/composer/ComposerTextarea.tsx#L36-L48): `apply` 内の `resize(element)` が `maxHeight()` を生読みしている → `compute` に `maxHeight()` を含める。
   - [`FilesTool.tsx`](../../ui/src/lib/components/dock/FilesTool.tsx#L142-L245): トップレベルの `let lastOpenVersion = dock.fileOpenVersion;` に `untrack` がない。L148 の `isMobile()` / `dock.activeFile` が `apply` 内で読まれている。また `loadDir` が `{ ...dirs() }` スプレッドのため並列ロード時に先着結果を上書きする（関数アップデータ `setDirs((prev) => ...)` を使う）。
   - [`ElicitationCard.tsx`](../../ui/src/lib/components/ElicitationCard.tsx#L243): `<For>` コールバック本体での `props.elicitationId` 読み取りをアクセサ化する。
   - [`NodeSettings.tsx`](../../ui/src/routes/settings/NodeSettings.tsx#L39-L46): リダイレクト effect の `apply` で `nodeId()` を生読みしている → `compute` に含める。
   - 設定パネル群の `apply` 内 untracked read と非同期競合:
     - [`AgentManagementPanel.tsx`](../../ui/src/lib/components/settings/AgentManagementPanel.tsx#L39-L60): `loading()` ガードでキー切替時の再取得がスキップされ、generation 管理もないため古いレスポンスが上書きされる。
     - [`AgentManager.tsx`](../../ui/src/lib/components/settings/AgentManager.tsx#L126-L177): `onSettled` と `createEffect` の二重初回ロード、`nodeId` 切替時の世代チェック漏れ、エラー toast の出し分け不備、mutation 後の `loadAgents()` が `inFlight` で握り潰される問題。
     - [`FolderPicker.tsx`](../../ui/src/lib/components/fs/FolderPicker.tsx#L60-L102): `apply` での `props.nodeId` / `props.onloadingchange` 読み取り、および effect cleanup で `loadGeneration` を進めないため閉じた後にコールバックが発火する問題。
     - [`AuditLog.tsx`](../../ui/src/lib/components/settings/AuditLog.tsx#L26-L50), [`PairingPanel.tsx`](../../ui/src/lib/components/settings/PairingPanel.tsx#L53-L92), [`SettingsCatalog.tsx`](../../ui/src/lib/components/settings/SettingsCatalog.tsx#L93-L151): `compute` から対象パラメータ（`nodeId`, `section`, `isHub` 等）を受け取り、世代/ターゲット一致検証と cleanup を統一する。

---

### 1.5 ルーティング・フォーム・UX リグレッション（Critical〜Major）

| 対象                                                                                                                                                                                                                                                                  | 問題                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | 修正方針                                                                                                                                                                                   |
| :-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | :--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | :----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`settings-routes.ts`](../../ui/src/lib/settings-routes.ts#L38) / [`L98-106`](../../ui/src/lib/settings-routes.ts#L98-L106), [`NodeSettings.tsx`](../../ui/src/routes/settings/NodeSettings.tsx#L60)                                                                  | `isSettingsIndexPath()` が `"/settings/nodes"` (`NODES_BASE`) を含んでいるため、Hub でオンラインノードが 0 件のときに `SettingsShell` がクロームも `no_online_nodes` カードも隠してしまい、`NodeSettings` も何も描画せず**白紙ページ**になる。また `SETTINGS_ROOT` / `HUB_BASE` / `BROWSER_BASE` の末尾スラッシュ表記揺れも `false` になる。さらに `NodeSettings.tsx:60` に `params.tab !== undefined` ガードが無く、`/settings/nodes/:id` 遷移時に一瞬 `<SettingsCatalog>` がマウントされる。 | `isSettingsIndexPath` から `NODES_BASE` を外し（`settings-routes.test.ts:50` も修正）、末尾スラッシュの扱いを統一する。`NodeSettings.tsx` にも `params.tab !== undefined` ガードを入れる。 |
| [`ElicitationCard.tsx`](../../ui/src/lib/components/ElicitationCard.tsx#L278-L299)                                                                                                                                                                                    | カンマ区切り `array` 入力で入力のたびに末尾カンマ・空白を落とした文字列を `value` へ書き戻すため、2 つ目の要素を入力できない。`number`/`integer` も `"-"`, `"1.0"` 等の入力途中で `NaN` / `"1"` になり編集が壊れる。                                                                                                                                                                                                                                                                           | 入力中はローカル文字列ドラフトを保持し、配列・数値への確定変換は `onChange`/`onBlur`（または送信時バリデーション付き同期）で行う。                                                         |
| [`ExternalSessionsDialog.tsx`](../../ui/src/lib/components/session/ExternalSessionsDialog.tsx#L149-L154)                                                                                                                                                              | `HISTORY_UNAVAILABLE` 発生時に `origin/master` にあった「履歴なしで取り込む」再試行アクションが、新 `toast` にアクションボタンが無いため失われている。                                                                                                                                                                                                                                                                                                                                         | §1.6 の `ToasterStore` アクションボタン実装（[`docs/backlog.md`](../backlog.md) §8）またはダイアログ内の再試行ボタンとして復旧する。                                                       |
| [`FolderBrowserDialog.tsx`](../../ui/src/lib/components/fs/FolderBrowserDialog.tsx#L24-L49)                                                                                                                                                                           | クローズ時に `pickerPath` をクリアしないため、別ノードで開いた際（特に初期 `browseFs` 失敗時）に前ノードのパスが残る。                                                                                                                                                                                                                                                                                                                                                                         | `close()` / `onOpenChange(false)` で `setPickerPath("")` もリセットする。                                                                                                                  |
| [`AddProjectDialog.tsx`](../../ui/src/lib/components/projects/AddProjectDialog.tsx#L169-L179) / [`L342-346`](../../ui/src/lib/components/projects/AddProjectDialog.tsx#L342-L346) / [`L474-476`](../../ui/src/lib/components/projects/AddProjectDialog.tsx#L474-L476) | ① `FolderPicker` の `onerror` が `targetPath` を `""` にすると、preview `createEffect` が即座に `setPreviewError(null)` でエラー表示を消す。② `alreadyAdded() && advanced() && customKey().trim() === ""` のとき送信ボタンが活性のまま無動作になる。                                                                                                                                                                                                                                           | フォルダ読込エラーは専用 state に分離するか `targetPath === ""` 時のクリア条件を見直す。ボタンの `disabled` 条件と `submit()` ガードを一致させる。                                         |
| [`WorktreeManagerDialog.tsx`](../../ui/src/lib/components/projects/WorktreeManagerDialog.tsx#L90-L100) / [`L221`](../../ui/src/lib/components/projects/WorktreeManagerDialog.tsx#L221)                                                                                | Worktree 作成時に `createNodeId() === ""` をチェックせず空 `node_id` で送信できてしまう。                                                                                                                                                                                                                                                                                                                                                                                                      | ボタン `disabled` と `submitCreateWorktree` 双方に `createNodeId() === ""` ガードを追加する。                                                                                              |
| [`Composer.tsx`](../../ui/src/lib/components/chat/Composer.tsx#L118-L131)                                                                                                                                                                                             | `toggleAutoApprove` に `try/catch` がなく、例外時に `optimisticPolicy` が元に戻らない。                                                                                                                                                                                                                                                                                                                                                                                                        | `try/catch` で囲み例外時は `setOptimisticPolicy(null)` と問題報告／通知を行う。                                                                                                            |
| [`composer-attachments.ts`](../../ui/src/lib/composer-attachments.ts#L10-L19)                                                                                                                                                                                         | `<Toaster>` が `AppShell` にマウント済みにもかかわらず、ファイルサイズ超過等のユーザー操作ミスが永続的な `problems.warn` へ送られている（`origin/master` は `toast.error`）。                                                                                                                                                                                                                                                                                                                  | `toast.error(message)` に戻し、古いコメントを削除する。                                                                                                                                    |

---

### 1.6 UI プリミティブ層 (`ui/src/lib/components/ui/`) の修正・共通化（Critical〜Major）

1. **フォーム誤送信防止 (`type="button"`)**:
   - [`select.tsx`](../../ui/src/lib/components/ui/select/select.tsx#L135-L149), [`popover.tsx`](../../ui/src/lib/components/ui/popover/popover.tsx#L61-L70), [`dropdown-menu.tsx`](../../ui/src/lib/components/ui/dropdown-menu/dropdown-menu.tsx#L113-L123), [`tooltip.tsx`](../../ui/src/lib/components/ui/tooltip/tooltip.tsx#L97-L105): いずれも `<button>` にデフォルト `type="button"` を渡していないため、[`AgentManager.tsx`](../../ui/src/lib/components/settings/AgentManager.tsx#L387) の `<form>` 内で `<Select.Trigger>` をクリックするとフォームが送信される。4 つすべてに `type={props.type ?? "button"}` を設定する（`"type"` を `omit` し Props にも追加）。
2. **`Tabs` の非制御・状態スタイル修復**（[`tabs.tsx`](../../ui/src/lib/components/ui/tabs/tabs.tsx#L31-L135)）:
   - `Tabs.Root` が `defaultValue` を `value` に流すため非制御 `<Tabs.Root defaultValue="...">` が切替不能。`createControllable` を使うか `defaultValue` を Terracotta `<TabGroup>` へ正しく渡す。
   - `Tabs.Trigger` は `<div>` なので `disabled:` ではなく `[&[tc-disabled]]:pointer-events-none [&[tc-disabled]]:opacity-50` を使う。`TriggerProps` の `onClick` 欠落も直す。
   - `Tabs.Content` に `[&:not([tc-selected])]:hidden` を追加し `unmount={false}` 時に非選択タブが全表示されないようにする。
3. **`Sheet` / `Drawer` / `Dialog` / `AlertDialog` の修正と共通化**:
   - [`sheet.tsx`](../../ui/src/lib/components/ui/sheet/sheet.tsx#L165-L198): `<Portal>` へ入れる。
   - [`drawer.tsx`](../../ui/src/lib/components/ui/drawer/drawer.tsx#L33-L46) & [`drawer/index.ts`](../../ui/src/lib/components/ui/drawer/index.ts#L44-L56): `Drawer.Content` のデフォルトを `side="bottom"` に直し、`drawer/index.ts` の prop 型エクスポート先を `./drawer.tsx` に直す。
   - [`dialog.tsx`](../../ui/src/lib/components/ui/dialog/dialog.tsx), [`alert-dialog.tsx`](../../ui/src/lib/components/ui/alert-dialog/alert-dialog.tsx), [`sheet.tsx`](../../ui/src/lib/components/ui/sheet/sheet.tsx): 重複しているモーダル基盤（スクロールロック・フォーカス制御・`Trigger`/`Close`）を共通ヘルパー（例: `modal.ts`）に抽出し、以下を解消する。
     - 参照カウント付き `body.style.overflow` スクロールロック（ネストしたモーダルの開閉順序違いによる永久スクロールロック防止）。
     - Terracotta の `useFocusStartPoint` と二重化・競合している独自 `restoreFocus` の整理（制御 `open` 更新時も破綻しない単一のフォーカス管理へ）。
     - `AlertDialogOverlay` のクリック閉じ抑止（確認ダイアログが背景クリックで閉じない本来の仕様を維持、特に処理中 `busy` 時）。
     - 無視されている `disabled` prop、`Title`/`Description` での `ref` 転送漏れ、`Trigger`/`Close`/`Cancel` での `event.defaultPrevented` 無視の修正。
4. **`createControllable` の改修**（[`controllable.ts`](../../ui/src/lib/components/ui/controllable.ts#L17-L34)）:
   - 初回 `value() !== undefined` で `controlled` を固定する仕様を改め、制御／非制御を動的に判定する（`value() !== undefined` を基準にし、かつコールバックもアクセサまたは関数で受け取ることでトップレベルでの `props.onOpenChange` / `props.defaultOpen` 生読み取り＝`STRICT_READ_UNTRACKED` を無くす）。
   - 非制御時に `undefined` / `null` へ戻せるよう `createSignal(untrack(fallback))` で初期化する。
5. **`Input` / `Textarea` の非制御対応**（[`input.tsx`](../../ui/src/lib/components/ui/input/input.tsx#L23-L41), [`textarea.tsx`](../../ui/src/lib/components/ui/textarea/textarea.tsx#L20-L38)）:
   - `"onInput"` を `omit` に追加する。
   - 無条件の `value={props.value ?? ""}` をやめ、`props.value !== undefined`（または `"value" in props`）のときのみ `value` をバインドし、非制御入力・`defaultValue`・`type="file"` で他 prop 変更時に値が消える現象（`UNCOMMITTED_INPUT_OVERWRITTEN`）を防ぐ。
6. **`position.ts` の幾何・ポップアップ制御修正**（[`position.ts`](../../ui/src/lib/components/ui/position.ts)）:
   - L112–113 の `read(options.side)` / `read(options.align)` を `untrack`（または初期設定のみ）にし `STRICT_READ_UNTRACKED` を解消。
   - `positionArrow` で `staticSide` 設定前に `left`/`right`/`top`/`bottom` の 4 辺をすべてクリアし、`flip()` 後の矢印崩れを防ぐ。
   - `autoUpdate` 開始時の手動 `update()` 重複呼び出し削除、`.then()` への `cancelled` ガード追加、cleanup 時の状態復旧、`compute` の同値比較導入、接続待ちリトライ対応。
   - `dismissOnOutsidePointer` の `[data-anchored-panel]` 判定を親子階層コンテキスト（`PanelTreeContext` 等）ベースにし、Popover 内の Select 等で外側クリックが効くようにする。
   - Terracotta 側 `Popover`/`Listbox` との二重 `restoreFocusOnClose` 整理、および `animate-in` 中の `Escape` / `ArrowDown` と無フォーカス要素 `Popover.Content` (`tabindex={-1}`) の挙動整理。
7. **`DropdownMenu` の整理**（[`dropdown-menu.tsx`](../../ui/src/lib/components/ui/dropdown-menu/dropdown-menu.tsx)）:
   - `Item.handleClick` の `closeMenu()` と Terracotta `PopoverPanel` 側の自動クローズによる二重 `onOpenChange(false)` 解消（`event.defaultPrevented` 尊重・`onFlush` 対応含む）。
   - `SubTrigger` / `SubContent` のイベント競合修正（`stopPropagation()`、Space `keyup` とクリックの二重トグル防止、SubTrigger クリックでの閉じる動作、`ArrowLeft` でサブメニューを閉じてフォーカス復帰、disabled スタイル、`onKeyDown` omit）。
   - `CheckboxItem` / `CheckboxGroup` / `RadioGroup` の非制御対応。
8. **`Select` / `Popover` / `Button` / `Tooltip` / `toast` の個別修正**:
   - [`select.tsx`](../../ui/src/lib/components/ui/select/select.tsx) / [`popover.tsx`](../../ui/src/lib/components/ui/popover/popover.tsx): `disabled={props.disabled === undefined ? undefined : toBoolean(props.disabled)}` とし `Root` の `disabled` 上書きを防止。
   - `Select.Item`: `registerLabel` を `createEffect` + cleanup（unregister）に変更し `value` 変更・アンマウントに対応、`props.children` フォールバックや `Select.Trigger` 側の `data-placeholder` 付与を追加。`ScrollButton` を `sticky` 配置にし `"onClick"` を `omit`（あわせて `<ul>` 直下の非 `<li>` 要素配置のマークアップ改善を検討）。
   - [`button.tsx`](../../ui/src/lib/components/ui/button/button.tsx#L15) / [`L118-L129`](../../ui/src/lib/components/ui/button/button.tsx#L118-L129): `href` 指定時に `role`/`tabindex` へ `undefined` を明示指定して呼び出し元属性を消さないよう修正し、`disabled` を `anchorRest` から除き `aria-disabled:` スタイルを追加。`""` の `href` も `<button>` に落とさない（`props.href !== undefined`）。
   - [`tooltip.tsx`](../../ui/src/lib/components/ui/tooltip/tooltip.tsx#L91) / [`L98-110`](../../ui/src/lib/components/ui/tooltip/tooltip.tsx#L98-L110): `props.ref` トップレベル生読み取り解消、`TriggerProps` への `onClick` / `ComponentProps<"a">` 追加、Closed 時の不要な `<Portal>` マウント抑制（`popover.tsx`, `dropdown-menu.tsx` も同様。`*.Portal` と `Content` の二重 Portal 整理を含む）。
   - [`sonner/toast.ts`](../../ui/src/lib/components/ui/sonner/toast.ts#L37-L48) / [`toaster.tsx`](../../ui/src/lib/components/ui/sonner/toaster.tsx#L55-L72): `dismiss()` 時の `clearTimeout`、`default` アイコン色調整、必要に応じた閉じボタン／アクション API。

---

### 1.7 ストア・ライフサイクル・テーマ・Terminal・Markdown（Major）

1. **`ownedWrite: true` 撤去と `ReactiveCell` / `SessionTimeline` 整備**:
   - [`sync-effect.test.ts`](../../ui/src/lib/stores/sync-effect.test.ts#L17-L26) が `createEffect` の第 1 引数（`compute`）内で `setActiveSession` を呼んでいる誤りを第 2 引数（`apply`）へ移し、[`reactive.ts`](../../ui/src/lib/stores/reactive.ts#L15-L31) の `Notifier` から `{ ownedWrite: true }` を削除する（アンマウント cleanup 自体は Solid 2 の `enterDisposal` により元々書き込み許可されている）。
   - `ReactiveCell.peek()` を追加し、[`resource.ts`](../../ui/src/lib/stores/resource.ts), [`dock.ts`](../../ui/src/lib/stores/dock.ts), [`problems.ts`](../../ui/src/lib/stores/problems.ts), [`composer-attachments.ts`](../../ui/src/lib/composer-attachments.ts) 内のミューテータが自己状態を読む際 `STRICT_READ_UNTRACKED` を出さないようにする。
   - `ReactiveMap.set` に `Object.is` ガードを追加し、また `unreadSessions` / `worktrees` 等の負荷が高い箇所はキー単位の購読設計を検討する。
   - [`timeline.ts`](../../ui/src/lib/sync/timeline.ts#L131-L181): `events` / `headEvents` / `streamKeys` の in-place 変更 + `touch()` を、更新時に配列をシャローコピーする形へ改め `createMemo` の `===` 比較と整合させる。`#streamKeys.touch()` も実際にキー集合が変化したときのみ通知する。
2. **`SyncStore` / `PreferencesStore` / `push.ts` / `connection.ts` の堅牢化**:
   - [`sync.ts`](../../ui/src/lib/stores/sync.ts#L367-L403) / [`L578-585`](../../ui/src/lib/stores/sync.ts#L578-L585) / [`L1061`](../../ui/src/lib/stores/sync.ts#L1061): `#timelines` を `ReactiveMap` 化するか `reset` 世代シグナルを持たせ、カーソル巻き戻しで `SessionPage` の `timeline` memo が孤立インスタンスを参照し続ける問題を防ぐ。WS `open`/`message` への `this.#ws !== ws` ガード追加、再接続/切断時の `clearStreams()` 呼び出し、`loadSessionEvents` の過去イベント誤未読判定防止、in-flight `refresh*` 競合ガード。
   - [`prefs.ts`](../../ui/src/lib/stores/prefs.ts#L16), [`session-prefs.ts`](../../ui/src/lib/session-prefs.ts#L242-L248) 等の `localStorage` 読み取りを `try/catch` で包む。
   - [`push.ts`](../../ui/src/lib/push.ts#L43-L125): `getRegistration()` 例外ガード、`applicationServerKey` 一致確認、サーバー登録失敗時のローカル購読ロールバック。[`connection.ts`](../../ui/src/lib/stores/connection.ts#L159-L178): 再 probe 失敗時の `role = null` リセット。
3. **ダークモード FOUC・`highlight.ts` テーマ不一致**:
   - [`index.html`](../../ui/index.html#L13-L25) のインラインスクリプトで `fxg:palette` に加えて `mode-watcher-mode`（+ `prefers-color-scheme: dark`）から `.dark` をペイント前に適用する。
   - [`mode.ts`](../../ui/src/lib/mode.ts#L28-L74): `typeof window.matchMedia === "function"` と `document` ガードを追加、`mode` を `createMemo` 化、`window` `"storage"` イベントによるタブ間同期を追加し、誤解を招くコメントを実態に合わせる。
   - [`highlight.ts`](../../ui/src/lib/highlight.ts#L44-L64): `prefersDarkMode()` 固定ではなく `mode() === "dark"` に連動させ（Browser Settings でのテーマ上書きを Shiki / CodeView / Markdown コードブロックに反映）、ローダー失敗時にキャッシュ Promise をクリアしてリトライ可能にする。
4. **`TerminalView`・`GhosttyWebAdapter`・`MarkdownText`**:
   - [`TerminalView.tsx`](../../ui/src/lib/components/terminal/TerminalView.tsx#L193-L270) & [`ghostty-adapter.ts`](../../ui/src/lib/components/terminal/ghostty-adapter.ts#L56-L147): コンテナ ref 取得後、`adapter.mount` と `onregister` は DOM 接続完了後の `onSettled` / `createEffect` で実行する。`GhosttyWebAdapter` に `#disposed` / `#mountToken` を追加し `init()` 完了前の `dispose()` / 再 `mount()` リークを防ぐ。`isActive` の `createMemo` 化、`workspaceKey()` / `client` の `compute` 移管、非表示切替時の `adapter?.clear()` 漏れを修正する。
   - [`MarkdownText.tsx`](../../ui/src/lib/components/chat/MarkdownText.tsx#L57-L78): cleanup で無条件に `clearTimeout(timer)` しているため 80ms スロットルがデバウンス化している問題を、cleanup ではなくアンマウント時の `onSettled` クリーンアップまたは時刻ベース制御へ修正する。

---

### 1.8 コードスメル・設定・DRY・ドキュメント整合（Minor〜Refactor）

1. **ビルド・Lint・テスト基盤**:
   - [`Dockerfile`](../../Dockerfile#L8-L9): `pnpm install --frozen-lockfile` の前に [`ui/pnpm-workspace.yaml`](../../ui/pnpm-workspace.yaml) をコピーする。
   - [`ui/.oxlintrc.json`](../../ui/.oxlintrc.json#L11): `ignorePatterns` を `"build/**"` に修正し、`vitest` プラグインを復元、残存 4 warning（[`ProvisioningLogCard.tsx:13`](../../ui/src/lib/components/ProvisioningLogCard.tsx#L13), [`service-worker.js:37`](../../ui/src/service-worker.js#L37), [`SessionPage.tsx:423,439`](../../ui/src/routes/SessionPage.tsx#L423)）を解消する。
   - [`ui/vitest.config.ts`](../../ui/vitest.config.ts): `restoreMocks: true` を復元し、テスト実行時の Solid 2 dev 診断警告（`STRICT_READ_UNTRACKED`, `EFFECT_WRITES_OWN_SOURCE`, `UNCOMMITTED_INPUT_OVERWRITTEN` 等）をテスト失敗として検知するセットアップを導入する。
   - UI プリミティブ層（`controllable`, `position`, `tabs`, `select`, `dropdown-menu`, `dialog`, `input`）の単体テストを追加する。
2. **JSX マークアップ・細かい UI 修正**:
   - [`ProblemCenter.tsx`](../../ui/src/lib/components/ProblemCenter.tsx#L128-L129) と [`TokenDialog.tsx`](../../ui/src/lib/components/TokenDialog.tsx#L79-L86) の JSX 改行で消えた半角スペースを `{" "}` 等で復元する。
   - ネイティブ `<button>` へ関数 prop を直渡ししている箇所（[`DockHost.tsx:49`](../../ui/src/lib/components/dock/DockHost.tsx#L49), [`DockRail.tsx:48`](../../ui/src/lib/components/dock/DockRail.tsx#L48), [`FilePreview.tsx:177`](../../ui/src/lib/components/dock/FilePreview.tsx#L177)）を `() => props.on...()` に改める。
   - [`KillSwitchButton.tsx:60`](../../ui/src/lib/components/KillSwitchButton.tsx#L60) の `AlertDialog.Cancel` に `disabled={busy()}` を追加、[`BrandIcon.tsx:49`](../../ui/src/lib/components/BrandIcon.tsx#L49) のライトモード色味調整、[`AgentManager.tsx:742`](../../ui/src/lib/components/settings/AgentManager.tsx#L742) の冗長 `<Show>` 整理。
3. **Solid 2 イディオム・DRY・コード分割の整理**:
   - 重い・配列を返す・複数回読まれる導出値（`FolderPicker`, `SessionPage`, `SearchPage`, `NewSessionChat` 等）を `createMemo` 化する。
   - 23 箇所のインライン `error instanceof Error ? error.message : String(error)` を [`errorMessage()`](../../ui/src/lib/api/error-messages.ts) へ寄せる。
   - 6 ルートの `document.title` 更新を `useDocumentTitle` ヘルパーへ統合する。
   - `DiffViewer.tsx:17`（`createEffect` → `onSettled`）、`button.tsx`/`tooltip.tsx`/`badge.tsx` のクロージャ `omit()` 解消と `class?: ClassValue` 許容、テンプレート文字列 `style` の `style={{ ... }}` 統一。
   - コールバック prop 命名（Svelte 由来の小文字 `onopenchange` 等 vs camelCase `onOpenChange`）を段階的に統一し、[`PickerPill.tsx`](../../ui/src/lib/components/composer/PickerPill.tsx#L14) 等の JSX スロットも `children()` 規約へ揃える。
   - 未使用コード・残骸削除: [`utils.ts`](../../ui/src/lib/utils.ts#L3-L10)（`WithoutChild` 等・`any` 削除）、[`tooltip-context.ts`](../../ui/src/lib/components/ui/tooltip/tooltip-context.ts#L10-L12)（`useTooltipConfig` 削除）、[`dock.ts`](../../ui/src/lib/stores/dock.ts#L5-L8)（`isDockToolId` を `registry.ts` から import）、[`service-worker.js`](../../ui/src/service-worker.js) の `urlBase64ToUint8Array` 重複、[`composer-drafts.ts:57`](../../ui/src/lib/stores/composer-drafts.ts#L57) の `$state` 言及削除。
   - 大規模コンポーネント（[`NewSessionChat.tsx`](../../ui/src/lib/components/session/NewSessionChat.tsx) 1,870 行等）を機能単位のフック／サブコンポーネントへ分割する。
4. **ドキュメント・リポジトリ設定の整合**:
   - [`ui/CONVENTIONS.md`](../../ui/CONVENTIONS.md#L50-L87): L50 の日本語表記を英語に統一、L58 の「`ref` callback が cleanup を返す」誤記を修正（`@solidjs/web` の `applyRef` は戻り値を無視する）、L85–87 の孤立したテーブル行を `Svelte habits → Solid 2` 表へ戻す。
   - [`docs/07-web-ui-and-pwa.md`](../07-web-ui-and-pwa.md#L9): `型付き paths` 記述を [`docs/backlog.md`](../backlog.md#L90) の決定（`settings-routes.ts` 集約）と一致させる。
   - [`mise.toml`](../../mise.toml#L25) と [`.github/workflows/ci-fmt.yml`](../../.github/workflows/ci-fmt.yml#L4) に残っている `Svelte` 記述を更新する。

---

## 2. 実装フェーズ（推奨進め方）

依存関係とユーザー影響の大きさから、以下の 4 フェーズで進める。

| フェーズ                                                | スコープ                                                                                                                                                                                                         | 主な狙い                                                                                             |
| :------------------------------------------------------ | :--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | :--------------------------------------------------------------------------------------------------- |
| **Phase A: 機能破壊の緊急修正**                         | §1.1〜§1.5 のうち Critical 項目（遅延フラッシュ、`/settings/nodes`、effect メモ化、`<For keyed>`、`ChatTimeline:161`、`ChangesTool`、`CodeView`、`ElicitationCard`、`Select.Trigger` 等 `type="button"`）        | 明らかな動作不能・画面白紙化・ツリー崩壊・誤送信を即座に解消する。                                   |
| **Phase B: リアクティビティ基盤とライフサイクル健全化** | §1.4 の残項目・§1.7（`ownedWrite: true` 撤去とテスト修正、`peek()`、`SessionTimeline`/`SyncStore`、`mode.ts`/FOUC/`highlight.ts`、`TerminalView`/`GhosttyWebAdapter`、`MarkdownText`、設定パネルの非同期レース） | Solid 2 の原則に沿った単一モデルへ統一し、`STRICT_READ_UNTRACKED` と非同期レース・リークを根絶する。 |
| **Phase C: UI プリミティブ改修・共通化・テスト整備**    | §1.6（モーダル共通化・スクロールロック、`controllable.ts`、`position.ts`、`Tabs`/`Sheet`/`Drawer`/`DropdownMenu`/`Select`/`Input`/`Textarea`/`Button`/`toast`、プリミティブ単体テスト）                          | ポップアップ・フォーム・モーダルの挙動を安定させ、プリミティブの回帰テストを敷く。                   |
| **Phase D: Smell 解消・CI/設定・ドキュメント仕上げ**    | §1.8（`Dockerfile`、`oxlint`/`vitest` 設定 + dev 診断失敗化、JSX 空白、`errorMessage`/`useDocumentTitle` DRY 化、`CONVENTIONS.md`・docs 整合、大規模ファイル分割）                                               | マージ前品質ゲート・CI 一貫性・コードの保守性を仕上げる。                                            |

---

## 3. 検証基準・完了条件

### 3.1 自動チェック

1. `pnpm --dir ui run check`（Paraglide 生成 + `tsc --noEmit`）がエラー 0 で通ること。
2. `pnpm --dir ui run lint` がエラー 0・**warning 0** で通ること。
3. `pnpm --dir ui run test` が全件パスし、かつ実行中の stderr に `[STRICT_READ_UNTRACKED]` / `[EFFECT_WRITES_OWN_SOURCE]` / `[UNCOMMITTED_INPUT_OVERWRITTEN]` 等の Solid 2 診断が出力されないこと。
4. `pnpm --dir ui run build`、`mise run fmt`（チェック）、`cargo check -p fxg-ui-assets -p fxg-api` が成功すること。
5. Docker イメージビルド（`ui/pnpm-workspace.yaml` を含むコンテキスト）が通過すること。

### 3.2 手動動作確認（実ブラウザ / dev サーバー）

- **Hub（オンラインノード 0 台）**: `/settings/nodes` が白紙にならず `no_online_nodes` カードと設定ヘッダーが表示されること。
- **Hub 外部セッション取込**: ダイアログ表示直後に一覧が取得でき、ノード切替で切替先ノードの一覧が出ること。`HISTORY_UNAVAILABLE` 時に履歴なし取込へ再試行できること。
- **新規セッション画面**: エージェント切替時に新エージェントのモデル設定で probe が走り、エージェント／モード選択直後のリロードで選択した値が復元されること。
- **エージェント設定画面**: カスタムエージェント作成フォーム内の `<Select.Trigger>` をクリックしてもフォーム送信が起きないこと。
- **セッション画面**:
  - WS 更新中も `FilesTool` の展開状態と `AgentUsage` タイマーが保持されること。
  - ワークスペースが異なるセッションへ切り替えたとき `ChangesTool` と `TerminalView` が正しく切り替わること。
  - 「さらに読み込む (`loadOlder`)」でスクロール位置が飛ばないこと。
  - ファイル行リンクから `CodeView` の対象行へスクロールすること。
  - `ElicitationCard` でカンマ区切り配列と負数/小数がそのまま入力できること。
- **テーマ / モーダル**: ダークモードで初回ロード時に白いフラッシュ（FOUC）が起きないこと。モバイル `Sheet` 上から削除 `AlertDialog` 等を開閉しても背景スクロールが復帰すること。

---

## 4. 実装状況

Phase A〜D とフォローアップ 3 項目（`service-worker.js` の base64url デコード共有化、コールバック prop の PascalCase 統一、`NewSessionChat.tsx` の分割）まで完了した。経緯は [`docs/changelog/2026-11/2026-11-19-solid2-ui-fixes-and-hardening.md`](../changelog/2026-11/2026-11-19-solid2-ui-fixes-and-hardening.md) を参照。
