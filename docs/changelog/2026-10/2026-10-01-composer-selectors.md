# Composer 設定セレクタ (モード / モデル) の表示一貫性改善

- **日付**: 2026-10-01
- **対象パッケージ**: `ui`
- **対象スクリプト / 設定**: `docs/05-cli-and-pwa-ui.md` §3

## 概要

チャット下部 (Composer) の設定ドロップダウンに以下の問題があった。

1. **何を選択するのか分かりにくい**: トリガには値しか表示されず、その
   セレクタがモードなのかモデルなのかを判別できなかった。
2. **表示の不一致 (値 vs 表示名)**: 未選択時や一覧をマウントしていない状態では
   `Select.Value` が生の値 (例: `default`) にフォールバックし、一覧と同じ項目が
   `Default` と表示されていた。
3. **接頭辞の不一致**: 設定項目の `Select.Item` に `label={"{name}: {value}"}`
   を渡していたため、トリガにだけ `Model: xxx` のような接頭辞が現れ、モード等の
   他セレクタと体裁が揃っていなかった。
4. **生値の露出**: `current_value` が `null` (opencode2 の未設定モデル) の場合、
   トリガに `null` が表示され得た。

## 実装

| ファイル                                           | 内容                                                                                                       |
| :------------------------------------------------- | :--------------------------------------------------------------------------------------------------------- |
| `ui/src/lib/components/chat/ComposerSelect.svelte` | 新規。項目名 (モード / モデル等) を固定表示するトリガ + `items` による安定したラベル解決                   |
| `ui/src/lib/config-options.ts`                     | `choiceLabel` が真偽値を `有効` / `無効` と表示。`selectedConfigValue` を追加 (選択肢に無い値は未選択扱い) |
| `ui/src/lib/components/chat/Composer.svelte`       | モード / 設定項目の `Select` を `ComposerSelect` へ置換。モード一覧には説明文を併記                        |
| `ui/src/lib/config-options.test.ts`                | 真偽値ラベルと `selectedConfigValue` のテストを追加                                                        |

### 表示ルール

- トリガは常に `項目名 現在値` (例: `モード Code` / `モデル Claude Sonnet`)
  の形式で表示する。項目名は muted 色で固定表示する。
- トリガの値と一覧の項目は同一のラベル (`items` の `label`) を使う。bits-ui の
  `Select.Root.items`
  を渡すことで、一覧が未マウントでも生値へフォールバックしない。
- 未選択 (`current_value` が `null` / 選択肢に存在しない)
  の場合は「既定」を表示する。
