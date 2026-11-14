# 2026-11-09 UI/UX 全体改善計画と実装記録

- **日付**: 2026-11-09
- **対象パッケージ**: `ui` (`fxg-ui-assets`)
- **対象スクリプト/コンポーネント**: `AppSidebar`, `Composer`, `NewSessionChat`, `ToolCallGroup`, `DiffPane`, `TerminalView`, `+layout.svelte`, `sessions/[id]/+page.svelte`

## 概要

既存のUIにおける挙動の不整合、配置・レイアウトの破綻、窮屈さ、ナビゲーションの不自然さを解消し、全体的な操作感（UX）と見た目の完成度を高める。新機能の無用な肥大化は避け、開発・運用における快適さと安定性を重視する。

## 実装計画

- [x] **Phase 1: Enter送信の整合性とIME対応**
  - 新規作成画面（`NewSessionChat`）とチャット画面（`Composer`）で送信キーの挙動が異なっていた問題を統一。
  - 送信設定（`Enter` で送信 / `Ctrl/Cmd+Enter` で送信）をユーザーが選択可能にし、設定を共通参照。
  - IME変換中（日本語等）の `isComposing` / `keyCode === 229` ガードをすべての入力欄に徹底。
- [x] **Phase 2: モバイルヘッダーとナビゲーション導線の整理**
  - セッション画面（`sessions/[id]`）の戻るボタンが常に新規作成画面（`/`）へ戻ってしまう挙動を改善。
  - モバイルトップバーでロゴと右端ボタンが両方 `/` を指していた冗長性を解消し、承認待ち（Inbox）やクイックアクションを適切に配置。
- [x] **Phase 3: Composer ツールバーの過密・横溢れ解消**
  - `ComposerCard` の `toolbar` が `flex-nowrap` で固定され、Diff/Terminalペイン分割時やモバイルで横溢れ・ボタン圧縮が起きる問題を解消。
  - ボタンの優先度付け（主要アクションと補助アクション）、アイコン化とツールチップの整理、レスポンシブな折り返し・オーバーフロー対応。
- [x] **Phase 4: ToolCallGroup（ツール呼び出しログ）の表示最適化**
  - 連続するツール呼び出しがすべて展開状態で表示され、チャットタイムラインが埋め尽くされる問題を改善。
  - 完了したツールグループはデフォルトで折りたたみ、サマリー（成功数・エラー有無）を表示。進行中・エラー時のみ展開する。
- [x] **Phase 5: デスクトップサイドバーの折りたたみ（Collapse）対応**
  - デスクトップ環境でサイドバーを折りたたんで作業領域（チャット、Diff、Terminal）を最大化できるようにする。
  - ショートカット（`Ctrl+B` / `Cmd+B`）およびトグルボタンの提供。
- [x] **Phase 6: Diff / Terminal 分割ペインのリサイズ機能**
  - デスクトップでの右ペイン（Diff / Terminal）の固定幅（5:5）をドラッグで幅調整（20%〜75%）できるように改善（キーボード操作対応、LocalStorage永続化）。
- [x] **Phase 7: 全体検証とビルド確認**
  - `pnpm --dir ui run check`（型検査: 0 errors / 0 warnings）
  - `pnpm --dir ui run test`（UIテスト: 10/10 files passed, 92/92 tests passed）
  - `pnpm --dir ui run build` & `cargo build --release -p fxg-cli`（バイナリ統合検証: 正常完了）

---

## 実装ログ / 進捗メモ

- 2026-11-09:
  - ブランチ `feat/ui-ux-improvements` 作成、作業計画策定。
  - **Phase 1 完了**: `session-prefs.ts` に `SendShortcut` 型・永続化関数および `shouldSubmitOnEnter` を追加。設定画面の Browser タブに送信ショートカット選択オプションを追加。`Composer.svelte` と `NewSessionChat.svelte` でショートカット判定を統一し、IME 誤送信防止ガードを共通化。プレースホルダーも設定に応じて動的表示。ユニットテスト 5 件追加。
  - **Phase 2 & Phase 5 完了**: `layout.svelte.ts` でモバイルドロワーとサイドバー折りたたみ状態を共通管理。セッション画面ヘッダーの左上ボタンでモバイルドロワーを開くように修正。モバイルトップバーで承認待ちがある時は Inbox バッジリンク、`/` 画面では検索リンクを動的表示するように改善。デスクトップでのサイドバー開閉ボタンおよび `Ctrl+B` / `Cmd+B` ショートカットを追加。
  - **Phase 3 完了**: `ComposerCard.svelte` のツールバーを `flex-wrap` に対応させ、Diff/Terminalペイン分割時や小画面幅での横溢れを解消。`Composer.svelte` のツールバー内ボタン（自動承認など）をレスポンシブなアイコン＋テキスト表示に最適化。
  - **Phase 4 完了**: `ToolCallGroup.svelte` で、正常完了したツール呼び出しグループを初期状態で自動折りたたみ表示に改善。進行中・失敗時のみ自動展開し、ユーザーの手動クリック操作を最優先保持するように実装。チャットタイムラインの縦長化・情報過多ノイズを大幅削減。
  - **Phase 6 完了**: `sessions/[id]/+page.svelte` にドラッグ可能なスプリッターリサイザーを追加。Diff や Terminal ペインの幅を 20%〜75% の範囲でドラッグ調整可能にし、矢印キーによるキーボード操作にも対応。設定幅は localStorage に自動保存・復元。
  - **新規作成画面の改善**: `NewSessionChat.svelte` に未承認（Inbox）リクエストの警告バナーを追加し、承認待ちの見落としを防止。
  - **Phase 7 完了**: `pnpm run check`（0 errors, 0 warnings）、`pnpm run test`（全92テスト通過）、`pnpm run build`、`cargo check`、`cargo build --release -p fxg-cli` の完全パスを確認。
