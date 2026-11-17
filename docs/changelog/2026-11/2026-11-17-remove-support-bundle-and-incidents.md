# サポートバンドルとインシデント永続化の削除

- **日付**: 2026-11-17
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-hub`, `fxg-node`, `fxg-cli`, `ui`, `docs`
- **対象スクリプト**: `mise run types`, `mise run sqlx:prepare`,
  `cargo test --workspace`, `cargo clippy --workspace --all-targets`,
  `pnpm --dir ui run check`, `pnpm --dir ui run lint`

---

## 概要

個人利用を前提とし、共有用のサポートバンドルと、その redaction 実装、
およびサーバー側のインシデント永続化を削除した。

redaction は「サポートバンドルを他人に共有できる形にする」ために必要な処理で
あり、共有を前提としなければ不要になる。エラーは tracing ログに残り、
UI のエラー通知はレスポンス / WebSocket から直接届くため、インシデントの
永続化を削除しても通知経路は変わらない。

## 変更

### 削除した機能

- `GET /api/v1/diagnostics/bundle` と `fxg support bundle`
- `GET /api/v1/diagnostics/incidents` と
  `POST /api/v1/diagnostics/incidents/:id/resolve`
- `fxg support incidents`
- `SupportBundle` / `SupportBundleConfig` / `DatabaseStats` /
  `SupportBundleSession` / `IncidentEntry` / `IncidentListResponse` /
  `IncidentLevel` / `IncidentSource` / `IncidentQuery`
- `RecoveryAction::ExportDiagnostics`
- `x-fxg-error-code` レスポンスヘッダ（クライアントは本文の `error.code` を読む）
- `incidents` テーブルと `fxg-db::incidents` モジュール
- REST / Client WS / ノード RPC / エージェントのエラー境界でのインシデント記録
- redaction 処理（トークン・Cookie・URL 資格情報・ホームディレクトリ・
  JSON の秘密キー）
- UI のサポートバンドル出力ボタンと「Recent incidents」一覧

### 残したもの

- `GET /api/v1/diagnostics`: コンポーネントヘルス（`db` / `migrations` /
  `http` / `sync` / `hub` / `disk`）、配信カウンタ、エージェント stderr、
  ログパス
- `fxg doctor` / `fxg logs`
- UI の Problem center とエラートースト（API / WS のエラーから直接構築）
- tracing ログと `request_id` による相関

### スキーマ

- `0001_initial.sql` から `incidents` テーブルとインデックスを削除した。
- プリリリースのため旧 DB との互換性は持たない。開発環境では
  `fxg internal wipe` で作り直す。
- `.sqlx` を再生成し、インシデント関連のクエリキャッシュを削除した。

## 検証

- `cargo check --workspace --all-targets`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `pnpm --dir ui run check`
- `pnpm --dir ui run lint`
