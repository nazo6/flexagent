# Web UI CLI 全機能同等化 (Feature Parity) 実装記録

- **日付**: 2026-10-01
- **対象パッケージ**: `fxg-protocol`, `fxg-server`, `fxg-node`, `ui`
- **対象スクリプト / 設定**: `docs/03-protocol-and-api.md`,
  `docs/05-cli-and-pwa-ui.md`

---

## 概要

CLI (`fxg`) で提供されている機能のうち、Web UI
で未対応だった以下の機能を段階的に実装し、Web UI 上で CLI
と同等の全操作を行えるようにする。

1. **フェーズ A: 既存 API の UI 統合 & セッション制御の完全化**
   - セッション強制終了 (Kill) UI
   - セッション会話分岐 (Fork) UI
   - セッション新規起動オプションの拡充 (`mode`, `opencode_mode`, `extra_args`)
   - プロビジョナー疎通テスト UI
2. **フェーズ B: セッション Revert (Shadow Git Tree 巻き戻し)**
   - Node ⇔ Server プロトコル & Client API の Revert 対応
   - Web UI (タイムライン / Diff) からのターン指定 Revert UI
3. **フェーズ C: プロジェクト & Worktree 高度管理**
   - Worktree Prune (クリーンアップ) UI & API
   - プロジェクト一括スキャン (Project Scan) UI & API
   - プロジェクト手動紐付け (Project Link) UI & API
4. **フェーズ D: エージェント管理 & ノードペアリング (セキュリティ)**
   - ACP Registry エージェント管理画面 (`/agents`)
   - エージェントのインストール / アップデート / 削除 API & UI
   - ノードペアリング & トークン管理 (`/settings`)
   - ノード個別トークンの発行 / 失効 / 一覧 & クライアントトークンローテーション

---

## 実装ログ

### フェーズ A: 既存 API の UI 統合 & セッション制御の完全化 (完了)

- [x] セッション強制終了 (Kill) ボタン & 確認モーダル (`/sessions/[id]`)
- [x] 会話分岐 (Fork) のタイムライン導線 & 新規セッション連携
      (`ChatTimeline.svelte`, `NewSessionChat.svelte`)
- [x] セッション作成フォームの詳細オプション (`mode`, `opencode_mode`,
      `extra_args`) をプロトコル・サーバー・ノード・UI に貫通
- [x] プロビジョナー接続テストのモーダル & ログ表示 (`NewSessionChat.svelte`,
      `testProvisioner` API)

### フェーズ B: セッション Revert (Shadow Git Tree 巻き戻し) (完了)

- [x] Node ⇔ Server プロトコルに Revert メッセージ
      (`ServerToNodeMsg::RevertSession`, `NodeToServerMsg::RevertResult`) 追加
- [x] Central Server / Local Node REST API (`POST /api/v1/sessions/:id/revert`)
      追加
- [x] Web UI (タイムライン) に巻き戻しボタン & 確認モーダル実装
      (`ChatTimeline.svelte` の各ユーザーターン、`/sessions/[id]` の Revert 確認
      ダイアログ)
- [x] 監査ログ `session_revert` を追加 (サーバー / ノード双方)
- [x] CLI の重複型 (`SessionRevertOutcome`) を `SessionRevertResponse` へ統合
      (DRY)

### フェーズ C: プロジェクト & Worktree 高度管理 (完了)

- [x] Worktree Prune (`git worktree prune`) の API & UI
      (`WorktreeAction::Prune`, `POST /api/v1/projects/:id/worktrees/prune`,
      プロジェクトカードの Prune ボタン & 確認ダイアログ)
- [x] プロジェクト一括スキャン (`fxg project scan`) の API & UI
      (`POST /api/v1/nodes/:node_id/projects/scan`, スキャンダイアログ)
- [x] プロジェクト手動紐付け (`fxg project link`) の API & UI
      (`POST /api/v1/nodes/:node_id/projects/link`, フォルダブラウザ連携)
- [x] 共有実装の集約 (`ops.rs` の `project_scan` / `project_link` /
      `worktree_prune`。IPC ハンドラも同じ実装を呼び出すようにリファクタ)
- [x] 監査ログ `worktree_prune` / `project_link` を追加

### フェーズ D: エージェント管理 & ノードペアリング (完了)

- [x] ACP Registry カタログ + ノード導入状態の統合 API (`GET /api/v1/agents`。
      中央サーバーは `ListAgents` をノードへ中継し、全ノードの
      `installed_agents` を統合)
- [x] エージェントのインストール / 更新 / 削除 API
      (`ManageAgent` のノード中継。`/api/v1/nodes/:id/agents/...`)
- [x] エージェント管理画面 (`/agents`。ノード選択・インストール・一括更新・削除)
- [x] ノードペアリング API (`GET/POST /api/v1/nodes/tokens`,
      `DELETE /api/v1/nodes/tokens/:node_id`。平文は発行時のみ返却)
- [x] クライアントトークン再生成 API (`POST /api/v1/auth/rotate-token`。
      サーバーは `RwLock` 化 + `auth_token` ファイルへ永続化)
- [x] 設定画面 (`/settings`。ノードトークン発行モーダル・失効・トークン再生成)
- [x] サイドバーに「エージェント管理」「設定」ナビゲーションを追加
- [x] 監査ログ `agent_manage` / `node_token_manage` / `auth_token_rotate` を追加
