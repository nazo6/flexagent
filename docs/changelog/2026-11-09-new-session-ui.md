# 新規セッション画面の composer 化とモデル事前選択

- **日付**: 2026-11-09
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `fxg-node`, `fxg-server`, `ui`
- **対象スクリプト**: `mise run types`, `mise run fmt`, `mise run check`

## 目的

新規セッション画面の設定カードが「Project / Node / Agent / Run environment /
Working directory / Advanced」に分散して分かりにくく、モデルもセッション開始前には
選べなかった。Composer 中心のレイアウトに再構成し、Suggested prompts を削除する。

## 変更内容

### UI

- `NewSessionChat.svelte` を composer 中心に再構成した。
  - 上段: Project / Working directory / Environment の pill。
  - 下段: Agent / Model / その他の `configOptions` の pill。
  - 詳細設定は歯車の `SessionSettingsDialog` に退避。
  - Suggested prompts と独立した Start empty ボタンを削除。
  - `Enter` で開始、`Shift + Enter` で改行（IME 変換中は開始しない）。
  - プロジェクト未登録・ノード未接続・パスなしは、入力欄の上に優先度の高い
    1件だけ callout を表示する。
- 追加コンポーネント:
  - `PickerPill.svelte`: Popover ベースの共通 pill メニュー。
  - `PickerItem.svelte`: 選択状態付きのメニュー項目。
  - `ConfigOptionPicker.svelte`: 検索付きのモデル / configOption 選択。
  - `SessionSettingsDialog.svelte`: mode / extra args / worktree / manual agent ID。
  - `ui/src/lib/components/ui/popover/*`: bits-ui Popover のラッパー。
- `config-options.ts`:
  - provider の副ラベル表示。
  - model → thought level → その他の順序。
  - `category = "mode"` は `CreateSessionRequest.mode` と重複するため除外。
  - 保存値が現在の選択肢に存在する場合だけ初期値に採用する
    `resolveInitialConfig` を追加。
- `en.json`: Suggested prompts 関連キーを削除し、モデル / セッション設定の
  文言を追加。

### プロトコル

- `ConfigOptionInfo` に `description` / `category` を追加。
- `ConfigOptionCategory`（open string enum）を追加。
- `AgentCapabilitiesPreview` / `ProbeAgentRequest` を追加。
- `NodeRpcRequest::ProbeAgentConfig` と
  `NodeRpcResponse::AgentCapabilities` を追加（timeout 120 秒）。
- `mise run types` で `ui/src/lib/generated/bindings.ts` を同期。

### ノード / サーバー

- `SessionManager::probe_agent_capabilities` を追加。
  - 既存 driver / warm pool で一時 `session/new` を実行。
  - `CapabilitiesUpdated` を取得して `shutdown`。`sessions` 行は作らない。
  - `(agent_id, local_path)` で約10分キャッシュし、同時実行を直列化。
  - 実セッションの `CapabilitiesUpdated` でもキャッシュを更新。
- `POST /api/v1/nodes/:node_id/agents/:agent_id/probe` を追加。
  `force: true` でノードキャッシュを迂回する（UI の Refresh）。
  `ClientApiBackend` の default 実装で `execute_node_rpc` を呼ぶため、
  ローカルノードと中央サーバーの両方で同じ経路になる。
- UI の `ApiClient.probeAgent` を追加。

## 制約・注意

- 一時VMはノード起動前のため、開始前のモデル一覧取得はできない。開始後に
  Composer から変更する。
- プローブはエージェント側に空セッションを残す実装がある。
- 初回プローブは `npx` / `uvx` のコールドスタートで数秒かかる。warm pool により
  実セッションの起動は速くなる。

## 検証

- `cargo test -p fxg-node probe_agent_capabilities_returns_options_and_caches`
- `pnpm --dir ui test`
- `mise run types`
- `mise run fmt`
- `mise run check`
