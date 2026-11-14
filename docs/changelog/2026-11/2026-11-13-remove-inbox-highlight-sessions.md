# 専用 Inbox 画面・API の廃止とセッション状態ベースの強調表示

- **日付**: 2026-11-13
- **対象パッケージ**: `fxg-acp`, `fxg-protocol`, `fxg-hub`, `ui`, `docs`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run types`, `mise run lint:errors`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
  `pnpm --dir ui run check`, `pnpm --dir ui run lint`, `pnpm --dir ui run test`
- **関連ドキュメント**:
  - [`docs/03-protocol-and-api.md`](../../03-protocol-and-api.md) §2.1
  - [`docs/05-cli-and-pwa-ui.md`](../../05-cli-and-config.md) §3.2 / §5.1

## 背景

`/inbox` 専用画面と `GET /api/v1/inbox` / `sync.inbox` / `sync.inboxElicitations` は、
承認（`permission_request`）と質問（`elicitation_request`）の双方を一覧画面向けに二重管理しており、
実際には質問だけ Inbox に表示されない不整合も生んでいた。
一覧画面そのものをなくし、各セッション自身の状態（`SessionSummary.status`）を単一のソースとして
サイドバーやナビゲーション上で強調表示し、対応は該当セッションのチャット内で直接行う構成に整理する。

## 変更

- **専用 Inbox 画面・同期経路の廃止**:
  - `fxg-protocol`: `InboxResponse` を削除し、`mise run types` で `ui/src/lib/generated/bindings.ts` を再生成。
  - `fxg-hub`: `GET /api/v1/inbox` ルートとハンドラを削除。
  - `ui`: `/inbox` ルート (`ui/src/routes/inbox/+page.svelte`)、サイドバー・モバイルヘッダーの Inbox リンク、`NewSessionChat` の Inbox バナー、`client.inbox()`、`sync.inbox` / `sync.inboxElicitations` / `refreshInbox`、および未使用の `inbox_*` / `shell_approval_inbox` i18n キーを削除。
  - Service Worker の `openApp` で `sessionId` がない場合の遷移先を `/inbox` から `/` に変更。
- **セッション状態ベースの強調表示 (`SessionSummary.status`)**:
  - `crates/fxg-acp/src/acp/client.rs`: 複数の承認・質問が同時に保留されている場合に片方だけを解決しても `Running` に戻らないよう `next_status_after_request` を導入（ユニットテスト追加）。
  - `ui/src/lib/session-status.ts`: `isWaitingSessionStatus` / `sessionNeedsAttention` / `sessionAttentionBadgeLabel` を追加（`session-status.test.ts` で検証）。待機状態（`waiting_permission` / `waiting_input`）はアーカイブ済みでも「Needs attention」に残し、`error` は未アーカイブのみ対象とする。
  - `ui/src/lib/components/layout/AppSidebar.svelte`: サイドバーの「Needs attention」セクションとプロジェクト待機件数バッジを `SessionSummary.status` ベースに統一し、セッション行の左アクセント、コンパクトピル（`Approval` / `Input` / `Error`）、ツールチップの状態表示を追加。
  - `ui/src/routes/+layout.svelte` / `ui/src/lib/components/session/SessionHeader.svelte`: モバイルのメニューボタンと折りたたみ時デスクトップサイドバーボタンに要対応件数バッジを表示。
- **設計ドキュメントの更新**:
  - `docs/03-protocol-and-api.md` と `docs/05-cli-and-pwa-ui.md` から `/inbox` 画面・API の記述を削除し、サイドバー注意喚起・チャット内インラインカード前提へ更新。

## 注意（変更しない不変条件）

- DB の `permission_requests` / `elicitation_requests` テーブルおよび投影、`SessionDetail.pending_permissions` / `pending_elicitations`、`allow_always` 昇格、Web Push 通知はそのまま維持する。

## 検証

- `pnpm --dir ui run check` (`svelte-check`, service-worker `tsc`): 0 errors, 0 warnings
- `pnpm --dir ui run lint` (`oxlint`): 0 errors, 0 warnings
- `pnpm --dir ui run test` (`vitest`): 14 test files / 126 tests passed
- `mise run lint:errors`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
