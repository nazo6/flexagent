# 2026-10-02 TUIの削除およびACPドライバへの一本化

- **日付**: 2026-10-02
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `fxg-db`, `fxg-node`,
  `fxg-server`, `fxg-cli`, `ui`
- **対象スクリプト / コマンド**: `fxg run`, `fxg session fork`,
  `fxg session resume`, `fxg session prompt`, `mise run check`

## 1. 概要・背景

エージェント統合と CLI 操作のシンプル化を目的に、以下のリファクタリングを実施：

1. **内蔵 TUI および `fxg attach` の削除**: Web UI / PWA に一本化し、`fxg-cli`
   から `ratatui`, `crossterm`, `unicode-width` への依存を全廃。セッション開始
   (`fxg run` / `fork` / `resume`) 時には自動的に Web UI のセッション URL
   をブラウザで開く仕様に変更 (`--no-open` または `-d` で抑制可能)。
2. **ACP への一本化**: OpenCode2 専用の HTTP/SSE ブリッジ
   (`crates/fxg-acp/src/opencode2.rs` 約 2,800 行) を完全削除。全エージェントを
   ACP (`AcpDriver`) 経由での起動・対話に統一。エージェント ID のハードコード
   (`OPENCODE2_ID` 等) も全廃し、ACP Registry またはローカル `PATH`
   からのコマンド解決に統一。
3. **IPC / Protocol メッセージの整理**: `AttachMode`, `AttachSession`,
   `SessionControlAction::Compact`, `opencode_mode`
   などの旧フィールドをプロトコル・DB・IPC・Web UI から削除。TypeScript
   型定義を自動更新。
4. **コンテキスト圧縮の整理**: Web UI
   の「圧縮」ボタンは専用エンドポイントの代わりに、エージェントが提供する
   `/compact` コマンドプロンプト送信ショートカットに変更。

## 2. 変更詳細

### 2.1 `fxg-protocol` & `ui/src/lib/generated/`

- `AttachMode`, `AttachSession` IPC メッセージを削除。
- `SessionControlAction::Compact` を削除。
- `CreateSessionRequest`, `ForkSessionRequest`, `SessionSummary` から
  `opencode_mode` を削除。
- `ts-rs` による TypeScript 型定義を同期。

### 2.2 `fxg-acp`

- `src/opencode2.rs` (2,882 行) を削除。
- `AgentDriver` トレイトから `revert_context`, `compact_context`,
  `native_attach` を削除。
- `OPENCODE2_ID` および旧コード用の特別扱いロジックを完全廃止。
- `which` クレートを導入し、`registry.rs` にて未登録エージェント ID をシステムの
  `PATH` (Windows では `PATHEXT` を考慮)
  からコマンド探索・起動する仕組みを実装。

### 2.3 `fxg-db`

- `searchable.rs`, `events.rs`, `tests/db_layer.rs` から `opencode_mode`
  カラムおよび参照を削除。

### 2.4 `fxg-server`

- `provisioner.rs`, `state.rs` から `opencode_mode` パラメータを削除。

### 2.5 `fxg-node`

- `session_manager.rs`: `opencode_mode`, `attach_mode`, `revert_context`,
  `Compact` 制御ロジックを削除。
- `daemon/ipc.rs`: `AttachSession`
  ハンドラ、イベントリプレイ、アタッチストリーム転送ロジックを削除。
- `daemon/http.rs`, `daemon/api.rs`, `sync.rs` などから `opencode_mode` を削除。

### 2.6 `fxg-cli`

- `src/tui.rs` を削除。
- `Commands::Attach` (`fxg attach`) を削除。
- `Cargo.toml` から `ratatui`, `crossterm`, `unicode-width` 依存関係を削除。
- `fxg run`, `fxg session fork`, `fxg session resume`
  時にブラウザでセッション画面を自動オープンする機能 (`opener::open_browser`)
  を追加。`--no-open`, `-d/--detach` フラグを追加。
- `fxg session prompt --wait` の追従処理を IPC アタッチ依存から `session_show`
  ポーリングへ移行。

### 2.7 `ui` (Web Frontend)

- `session-prefs.ts`: `OpencodeModePreference`, `opencodeMode` 設定項目を削除。
- `NewSessionChat.svelte`:
  `OpenCode2 実行モード` の選択肢を削除し、エージェント入力プレースホルダーを汎用化。
- `Composer.svelte`: `runControl({ action: 'compact' })` を `/compact`
  コマンドプロンプト送信に変更。

## 3. 検証結果

- `mise run check` (fmt check, cargo clippy, cargo test, ui
  fmt/lint/svelte-check/vitest) が全項目 PASS。
