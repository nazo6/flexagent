# 認証前メタ情報 `GET /api/v1/meta` の追加とトークン入力ダイアログの修正

- **日付**: 2026-10-02
- **対象パッケージ**: `fxg-protocol`, `fxg-server`, `ui`
- **対象スクリプト / 設定**: `mise run types` (ts-rs 型出力), `mise run check`

## 概要

Web UI の認証トークン入力ダイアログが、中央サーバー (`fxg server`) 接続時に
「ローカルノードのトークン」と誤表示し、確認方法の案内 (`fxg auth token`) も
事実と異なっていた問題を修正した。

原因は、認証前に接続先種別を知る手段が無く、UI が
「ループバック → ローカルノード、それ以外 → 中央サーバー」というホスト名推定
(`ConnectionStore.roleHint`) に依存していたこと。Docker compose 等で中央
サーバーを `http://localhost:8080` で開くと常に誤判定していた。

認証不要の `GET /api/v1/meta` (`{ role }` のみを返却) を追加。Web UI は接続先
種別の判定を `meta` のみで行うようにし (ホスト名推定・認証済み `system/info`
の `role` は使わない)、ダイアログの表示と確認方法の案内を正しく切り替える。
`meta` が取得できない場合は推測せず接続エラーとして扱う。

## 実装

| ファイル                                      | 内容                                                                                     |
| :-------------------------------------------- | :--------------------------------------------------------------------------------------- |
| `crates/fxg-protocol/src/client_api.rs`       | `MetaResponse` (`{ role }`、ts-rs 出力) を追加                                           |
| `crates/fxg-server/src/api/mod.rs`            | `META_PATH` (`/api/v1/meta`) のルートと `meta` ハンドラを追加                            |
| `crates/fxg-server/src/api/security.rs`       | `GET /api/v1/meta` のみ認証を免除 (Host / Origin 検証は通常どおり適用)                   |
| `crates/fxg-server/tests/meta_endpoint.rs`    | meta はトークン無しで 200 / Host 検証は維持 / 他 API は 401 のテストを追加               |
| `ui/src/lib/api/client.ts`                    | `meta()` を追加                                                                          |
| `ui/src/lib/stores/connection.svelte.ts`      | `role` を追加し、`meta` のみで接続先種別を判定 (取得失敗は接続エラー)                    |
| `ui/src/lib/components/TokenDialog.svelte`    | role 別に確認方法を表示 (サーバー: `auth_token` ファイル / Docker は `cat /data/...`)    |
| `ui/src/lib/components/ConnectionMenu.svelte` | 接続先ラベルを `role` ベースに変更 (接続不可時は「未接続」)                              |
| `ui/src/lib/components/*` / `ui/src/routes/*` | role 参照 (`systemInfo.role` / `roleHint`) を `connection.role` に統一                   |
| `docs/01`, `docs/03`, `docs/05`               | `/api/v1/meta` の仕様と、中央サーバーのトークン確認方法 (`fxg auth token` は不可) を追記 |

## 設計判断

- **認証免除は meta のみ**: 未認証の Web UI がトークン入力ダイアログを正しく
  表示するために必要な最小限の情報のみを返す。Host / Origin 検証は維持する
  (`docs/01` §7.3, `docs/03` §3.0)。
- **`fxg auth token` はデーモン IPC 専用のまま**(変更なし): 中央サーバーのみが
  動作するホスト / コンテナでは使用できないため、案内文から中央サーバーの
  確認方法としては削除し、`auth_token` ファイルの閲覧に統一した。
- **UI の role 判定は `meta` のみ**: 認証済み `system/info` の `role` や
  ホスト名推定は使わない。`meta` の取得失敗 (接続不可・非対応) は推測せず
  接続エラーとして表示する。UI は API と同じバイナリに同梱されるため、
  `meta` を持たないサーバーとの組み合わせは考慮しない (後方互換の
  フォールバックは持たない)。
