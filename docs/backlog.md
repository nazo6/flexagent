# Backlog（残タスク・技術的負債）

過去の実装計画（[`docs/proposals/`](./proposals/README.md)）から抽出した、未完了かつ今後も対応を検討するタスクの一覧です。

## 1. リファクタリング・モジュール分割

出典: [`docs/proposals/2026-11-09-refactoring-program.md`](./proposals/2026-11-09-refactoring-program.md)

- [ ] A1.5（保留）: セッションコマンド variant の統合
- [ ] 汎用ディスパッチャ化（REST の型付き RPC 化と合わせて判断）
- [ ] `crates/fxg-cli/src/commands.rs` の純粋分割（B4a）
- [ ] `fxg-db`: 1 対 1 の読み取りモデルを protocol DTO 直接返却へ移行、残る Row 構造体のマッピング整理、`metadata` 拡張カラムの適用範囲の文書化

## 2. 型安全性（ID newtype・allow 削減）

出典: [`docs/proposals/2026-11-08-type-safety-plan.md`](./proposals/2026-11-08-type-safety-plan.md) / [`docs/06-type-safety.md`](./06-type-safety.md)

- [ ] Tier C 相当の ID newtype 化検討（`PermissionRequestId`, `ElicitationId`, `AgentId`）
- [ ] 残存しているファイル単位 `#![allow(clippy::disallowed_types)]` / crate ルート `wildcard_enum_match_arm` allow の段階的削除

## 3. エラー処理・診断性の残項目

出典: [`docs/proposals/2026-11-08-error-handling-and-recovery-plan.md`](./proposals/2026-11-08-error-handling-and-recovery-plan.md) / [`docs/changelog/2026-11/2026-11-10-diagnostics-incidents.md`](./changelog/2026-11/2026-11-10-diagnostics-incidents.md)

- [ ] `fxg node status --verbose` / `stale_registry_served` の計測
- [ ] `PreflightFailed { checks: [...] }` による起動前チェックの標準化
- [ ] 部分成功レスポンス（`warnings: Vec<WarningInfo>` / `DataSafety`）の全境界への展開
- [ ] `NodeHello` でのプロトコル / DB スキーマバージョン互換チェック強化

## 4. ACP 追加機能（未対応項目）

出典: [`docs/proposals/2026-10-02-acp-enhancement-plan.md`](./proposals/2026-10-02-acp-enhancement-plan.md)

- [ ] `ToolCallContent::Terminal` の `terminal_id` を `ToolCall` に保持し、UI のツールカードからターミナル出力へリンクする
- [ ] `session.config_options.boolean` capability 広告・`terminal/output` の `truncated` 対応・`NewSessionRequest.additional_directories`・`session/close` 送信
- [ ] `AgentMessageChunk` / `AgentThoughtChunk` の Text 以外のコンテンツ対応

## 5. Web ターミナル（workspace スコープ化の残項目）

出典: [`docs/proposals/2026-11-14-terminal-workspace-scope.md`](./proposals/2026-11-14-terminal-workspace-scope.md)

- [ ] プロジェクト画面から workspace のターミナルを開く導線と、リロード後の自動復元（`GET /api/v1/terminals` を UI で利用する）
- [ ] worktree 削除時に、そのパスの workspace ターミナルを連動して kill する
- [ ] 複数ターミナル（タブ）、共有中バッジ、read-only モード、複数クライアントの resize 調停
- [ ] 無操作 TTL とリングバッファ上限を `node_settings` で変更可能にする（現状は定数）
- [ ] 終了済みターミナルの直近 1 世代（exit code + scrollback）を表示する
- [ ] UI から明示的にシェルを起動し直す「New shell」操作（プロトコルは `open { new: true }` に対応済み）
