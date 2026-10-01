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

### フェーズ B: セッション Revert (Shadow Git Tree 巻き戻し) (着手)

- [ ] Node ⇔ Server プロトコルに Revert メッセージ
      (`ServerToNodeMsg::RevertSession`, `NodeToServerMsg::RevertResult`) 追加
- [ ] Central Server / Local Node REST API (`POST /api/v1/sessions/:id/revert`)
      追加
- [ ] Web UI (タイムライン / Diff ペイン) に巻き戻しボタン & 確認モーダル実装
