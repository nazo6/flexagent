# 新規セッションの実行ディレクトリ既定解決 実装記録

- **日付**: 2026-10-01
- **対象パッケージ**: `fxg-db`, `fxg-server`, `fxg-node`, `ui`
- **対象スクリプト / 設定**: (なし)

---

## 概要

新規セッション作成 UX の以下の課題を改善する。

1. 実行ディレクトリが「ブランチ / Worktree 設定」の詳細設定に隠れており、
   未指定時の開始ボタンが無言で disabled になっていた
2. セッション開始時に `project_node_bindings` が登録・更新されないため、
   「そのプロジェクトをそのノードで実行した場所」が既定候補にならなかった

既定パスの解決順序は **直近使用した場所を優先**（Worktree 含む）:

1. `(project_id, node_id)` の紐付けのうち `last_used_at` が最も新しいもの
   （同時刻はメインリポジトリ優先）
2. 紐付けが無ければ `(project_id, node_id)` の最新セッションの `local_path`

---

## 実装ログ

### バックエンド: 既定解決の共通化

- [x] `fxg-db`: `queries::resolve_default_local_path` /
      `select_default_binding_path` を追加（紐付け → セッション履歴の順）
- [x] `fxg-db`: `Db::resolve_default_local_path` ラッパー
- [x] `fxg-server`: `ServerState::project_local_path` を共通実装へ置換
      （`local_path` 省略時の既定解決で、セッション履歴フォールバックも有効化）
- [x] `fxg-node`: ローカル REST `POST /api/v1/sessions` で `local_path` /
      `worktree` とも未指定の場合に既定解決し、無ければ 404 の明確なエラーを返す

### バックエンド: セッション開始時の紐付け登録

- [x] `fxg-node`: REST セッション開始時に `resolve_and_register_project`
      を呼び、
      `last_used_at` / ブランチを最新化（Worktree 同時作成は既存登録に任せる）
- [x] `fxg-node`: Context Fork 開始時も Fork 元の実行ディレクトリを最新化
- [x] `fxg-node`: ハブ経由 `StartSession` ハンドラで実行ディレクトリを登録
      （`NodeHello` 経由で中央サーバーへ伝播）
- [x] `fxg-node`: CLI `EnsureSession`（`fxg run`）でも同様に登録

### UI: 実行ディレクトリの第一級化

- [x] `ui/src/lib/new-session.ts`: 候補組み立て（紐付け + 直近セッション・
      同一パスは紐付け優先・直近使用順）、既定ノード選択、最終使用時刻の共通化
- [x] `ui/src/lib/new-session.test.ts`: 候補順序・重複・上限・ノード選択のテスト
- [x] `NewSessionChat.svelte`:
  - 「実行ディレクトリ（既存 / 新規 Worktree）」をメインフォームへ昇格
    （新規 Worktree のブランチ名も必須項目として表示）
  - 候補ゼロ時は「未登録」コールアウト（フォルダ選択 / プロジェクト設定 /
    登録がある別ノードへの切り替え）を表示
  - 未指定の理由（`pathRequiredHint`）を開始ボタン付近に明示
  - 候補の出典（登録済み / 前回のセッション）と最終使用時刻を表示
  - プロジェクト変更時は既定ノード・既定パスを再解決、ノード変更時は
    パスを再解決（クロス OS のパス持ち越しを防止）
  - URL `?node=` / `?path=` の初回適用（ディープリンク）と、
    `?project=` / `?agent=` / `?fork_*`
    の一度きり適用化（手動変更の巻き戻り防止）
- [x] `NewSessionChat.svelte`: 「ブランチ / Worktree 設定」→「詳細設定」に整理
      （起点ブランチ・配置先・起動オプションのみ残す）
- [x] `projects/+page.svelte`: 紐付け行に「この場所で新規セッション」を追加
- [x] `sessions/[id]/+page.svelte`: ヘッダに「ここで新規」を追加し、
      Fork リンクにも `node` / `path` を引き継ぎ

### 検証

- [x] `cargo test -p fxg-db --test db_layer`（既定解決の順序・フォールバック）
- [x] `cargo test -p fxg-node ipc_session_ensure_attach_prompt_and_permissions`
      （EnsureSession 後の紐付け登録）
- [x] `pnpm --dir ui run check` / `lint` / `test`
