# Proposals（設計提案・実装計画・技術調査）

複数クレートに跨がる設計変更、破壊的変更、アーキテクチャの比較検討（ADR）、複数フェーズの実装計画、技術調査メモを格納します。

## 運用ルール

1. **ライフサイクル**:
   - `Draft` → `In Progress` → `Implemented`（または `Partially Implemented` / `Superseded`）
2. **ステートへの昇格と凍結**:
   - 実装が完了し、確定した仕様・不変条件をステート docs（`docs/01`〜`07`, `troubleshooting.md`, `development.md`）へ反映した時点で、ステータスを `Implemented` にして凍結します。
   - 以降の仕様変更はステート docs 側を更新し、完了済みの Proposal は当時の判断記録として保持します。
3. **残タスクの扱い**:
   - 実装を部分完了で区切った場合、残っている有効なタスクは [`docs/backlog.md`](../backlog.md) へ集約します。

## 一覧

| ファイル                                                                                                                 | タイトル                                                      | 状態                    | 主な昇格先                               |
| :----------------------------------------------------------------------------------------------------------------------- | :------------------------------------------------------------ | :---------------------- | :--------------------------------------- |
| [2026-09-30-implementation-plan.md](./2026-09-30-implementation-plan.md)                                                 | FlexAgent (`fxg`) 段階的実装プラン & 進捗記録                 | `Implemented`           | `01`〜`05`, `07`                         |
| [2026-10-01-antigravity-startup.md](./2026-10-01-antigravity-startup.md)                                                 | `antigravity-acp` の起動遅延調査とプロセス再利用設計メモ      | `Implemented`           | `04` §5                                  |
| [2026-10-01-session-resume.md](./2026-10-01-session-resume.md)                                                           | セッション・レジューム機能 実装案                             | `Implemented`           | `04` §4.1                                |
| [2026-10-02-acp-enhancement-plan.md](./2026-10-02-acp-enhancement-plan.md)                                               | ACP 追加機能 実装計画（Phase 1〜5）                           | `Partially Implemented` | `03`, `04`, `backlog.md`                 |
| [2026-11-08-english-code-and-i18n-policy.md](./2026-11-08-english-code-and-i18n-policy.md)                               | コードの英語化・言語ポリシーの明文化                          | `Implemented`           | `development.md`, `AGENTS.md`            |
| [2026-11-08-error-handling-and-recovery-plan.md](./2026-11-08-error-handling-and-recovery-plan.md)                       | エラー処理・診断性・復旧性の改善プラン                        | `Partially Implemented` | `troubleshooting.md`, `03`, `backlog.md` |
| [2026-11-08-type-safety-plan.md](./2026-11-08-type-safety-plan.md)                                                       | 型安全性改善プラン (`serde_json::Value` / 文字列判定の排除)   | `Partially Implemented` | `06-type-safety.md`, `backlog.md`        |
| [2026-11-09-daily-driver-gaps.md](./2026-11-09-daily-driver-gaps.md)                                                     | 日常開発利用に向けたギャップ分析と実装計画                    | `Implemented`           | `04`, `05`, `07`                         |
| [2026-11-09-refactoring-program.md](./2026-11-09-refactoring-program.md)                                                 | リファクタリング・プログラム（API統合・モジュール分割）       | `In Progress`           | `01`, `03`, `04`, `backlog.md`           |
| [2026-11-09-settings-consolidation-and-project-identity.md](./2026-11-09-settings-consolidation-and-project-identity.md) | 設定の DB 集約・プロジェクト同一性・秘密ストア実装計画        | `Implemented`           | `01` §4, `02` §1.2, `05` §2              |
| [2026-11-10-architecture-simplification.md](./2026-11-10-architecture-simplification.md)                                 | アーキテクチャの単純化（HTTP 一本化・Git Askpass 撤去等）     | `Implemented`           | `01`, `02`, `05`                         |
| [2026-11-11-agent-extension-architecture.md](./2026-11-11-agent-extension-architecture.md)                               | エージェント実装の 3 層モデルと拡張基盤                       | `Implemented`           | `04` §1.1                                |
| [2026-11-11-node-pairing-db-and-gui.md](./2026-11-11-node-pairing-db-and-gui.md)                                         | ノード側ペアリングの DB 所有化と GUI 化                       | `Implemented`           | `01` §7.3, `02` §1, `05` §2              |
| [2026-11-13-data-directory-layout.md](./2026-11-13-data-directory-layout.md)                                             | データディレクトリ構成の刷新（OS 標準ルート + node/hub 分離） | `Implemented`           | `05` §2.1                                |
| [2026-11-14-terminal-workspace-scope.md](./2026-11-14-terminal-workspace-scope.md)                                       | ターミナルの workspace スコープ化と寿命分離                   | `Partially Implemented` | `03` §3.3, `07` §4                       |
