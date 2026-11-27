# Backlog（残タスク・技術的負債）

過去の実装計画（[`docs/proposals/`](./proposals/README.md)）から抽出した、未完了かつ今後も対応を検討するタスクの一覧です。

## 1. リファクタリング・モジュール分割

出典: [`docs/proposals/2026-11-09-refactoring-program.md`](./proposals/2026-11-09-refactoring-program.md)

- [ ] A1.5（保留）: セッションコマンド variant の統合
- [ ] 汎用ディスパッチャ化（REST の型付き RPC 化と合わせて判断）
- [ ] `fxg-db`: 1 対 1 の読み取りモデルを protocol DTO 直接返却へ移行、残る Row 構造体のマッピング整理、`metadata` 拡張カラムの適用範囲の文書化

## 2. 型安全性（ID newtype・allow 削減）

出典: [`docs/proposals/2026-11-08-type-safety-plan.md`](./proposals/2026-11-08-type-safety-plan.md) / [`docs/06-type-safety.md`](./06-type-safety.md)

- [ ] Tier C 相当の ID newtype 化検討（`PermissionRequestId`, `ElicitationId`, `AgentId`）
- [ ] 残存しているファイル単位 `#![allow(clippy::disallowed_types)]` / crate ルート `wildcard_enum_match_arm` allow の段階的削除

## 3. エラー処理・診断性の残項目

出典: [`docs/proposals/2026-11-08-error-handling-and-recovery-plan.md`](./proposals/2026-11-08-error-handling-and-recovery-plan.md)

incidents とサポートバンドルは 2026-11-17 に削除済み
（[changelog](./changelog/2026-11/2026-11-17-remove-support-bundle-and-incidents.md)）。

- [ ] `fxg node status --verbose` / `stale_registry_served` の計測
- [ ] `PreflightFailed { checks: [...] }` による起動前チェックの標準化
- [ ] 部分成功レスポンス（`warnings: Vec<WarningInfo>` / `DataSafety`）の全境界への展開
- [ ] `NodeHello` での DB スキーマバージョン互換チェック（プロトコルバージョンは実装済み）

## 4. ACP 追加機能（残項目）

出典: [`docs/proposals/2026-10-02-acp-enhancement-plan.md`](./proposals/2026-10-02-acp-enhancement-plan.md)

- [x] `ToolCallContent::Terminal` の `terminal_id` を `ToolCall` に保持し、UI のツールカードからターミナル出力へリンクする
- [x] `session.config_options.boolean` capability 広告
- [x] `terminal/output` の `truncated` 対応
- [ ] `NewSessionRequest.additional_directories`（`session/close` は送信済み）
- [x] `AgentMessageChunk` / `AgentThoughtChunk` の Text 以外のコンテンツ対応

## 5. Web ターミナル（workspace スコープ化の残項目）

出典: [`docs/proposals/2026-11-14-terminal-workspace-scope.md`](./proposals/2026-11-14-terminal-workspace-scope.md)

- [ ] プロジェクト画面から workspace のターミナルを開く導線（リロード後のタブ復元は実装済み。`GET /api/v1/terminals` の UI 利用が残り）
- [ ] worktree 削除時に、そのパスの workspace ターミナルを連動して kill する
- [x] 複数ターミナル（タブ、workspace + `terminal_id` で独立シェル）
- [ ] 共有中バッジ、read-only モード、複数クライアントの resize 調停
- [ ] 無操作 TTL とリングバッファ上限を `node_settings` で変更可能にする（現状は定数）
- [ ] 終了済みターミナルの直近 1 世代（exit code + scrollback）を表示する
- [ ] UI から明示的にシェルを起動し直す「New shell」操作（プロトコルは `open { new: true }` に対応済み）

## 6. Workspace Files の編集（Phase 2, TODO）

出典: [`docs/proposals/2026-11-14-workspace-dock.md`](./proposals/2026-11-14-workspace-dock.md) §6

現状は **読み取り専用** でマージする。編集は以下をまとめて実装する。

- [ ] 保存 API（`fs/write`）と監査ログ。既存ファイルのみを対象に、一時ファイル + rename で原子的に更新する
- [ ] 楽観ロック。読み込み時の `content_hash` と一致したときだけ保存し、不一致は `CONFLICT` を返す
- [ ] 競合 UI（上書き / 再読込）と、外部変更を検知したときのバナー
- [ ] Monaco エディタ、`Cmd/Ctrl+S`、未保存ドット、閉じる前の確認
- [ ] 保存後の Changes / Git status / ファイルプレビュー更新
- [ ] モバイルは閲覧のみとし、編集はデスクトップに限定する
- [ ] 編集 API も読み取りと同じくハブ経由で中継し、監査ログに記録する

## 7. エージェント端末の管理（Phase 1〜3, TODO）

出典: [`docs/proposals/2026-11-18-agent-terminal-management.md`](./proposals/2026-11-18-agent-terminal-management.md)

Phase 0（ACP v1 端末のライフサイクル・単体停止・スナップショット）は実装済み。

- [ ] MCP 端末サーバー（`terminal_create` / `terminal_read` / `terminal_kill` / `terminal_close`）をノードでホストし、`TerminalOutput` に正規化する
- [ ] MCP サーバーのエージェントへの注入（OpenCode の MCP 設定 / ACP エージェントの起動設定）と `agent_settings` への保存、UI からの有効化
- [ ] MCP 端末への入力（`terminal_send`）。cwd の封じ込め・env・permission ポリシーの検証を含む
- [ ] OpenCode の experimental persistent PTY を feature flag 付き・読み取り専用で「エージェント端末」として表示する
- [ ] ACP v2 のエージェント所有 Terminal Output を `TerminalOutput` に正規化する（停止可否は v2 の手段が決まってから）
- [x] `ToolCallContent::Terminal` の `terminal_id` を `ToolCall` に保持し、ツールカードから端末カードへリンクする（§4 と共通）
- [ ] 停止できない経路（自前実行・OpenCode ネイティブ）で「単体停止は不可」を UI に明示する

## 8. コンポーザー下書き（残項目）

出典: [`docs/proposals/2026-11-27-composer-drafts-and-sync.md`](./proposals/2026-11-27-composer-drafts-and-sync.md)

複数下書きの保存・切替・削除と、ハブ / ノードの REST による端末間共有は実装済み。
サイドバーの Drafts ビュー、リネーム、保存状態の表示・再試行、離脱時のフラッシュも実装済み。

- [x] サイドバーの Drafts ビューと下書きのリネーム
- [x] 保存状態（保存中 / 未保存）の表示、再試行、画面離脱時のフラッシュ
- [x] Client WS の変更通知（他端末の更新をリアルタイム反映する）
- [ ] 添付の共有（content-addressed blob とアップロード API、GC）
- [ ] node ⇔ hub の下書き複製（tombstone の伝播を含む）
- [ ] tombstone の保持期間 / GC と、下書きの件数・本文サイズ上限
