# 開発ガイド (`development.md`)

コントリビュータ・開発者向けのビルド・テスト・フォーマット・多言語化ワークフローをまとめます。

---

## 1. ツールチェインと `mise` タスク

ツールバージョンは [`mise.toml`](../mise.toml) で固定しています（`rust`, `node`, `pnpm`, `sqlx-cli`, `watchexec`, `tombi`）。

| タスク                                 | 用途                                                                                                    |
| :------------------------------------- | :------------------------------------------------------------------------------------------------------ |
| `mise run dev`                         | Vite dev server (`:5173`) と `fxg node start`（変更時自動リビルド）を並列起動                           |
| `mise run fmt` / `fmt:check`           | Rust (`rustfmt`), TOML (`tombi`), Markdown/JSON/YAML/TS/Svelte (`oxfmt`) の一括整形・検査               |
| `mise run lint`                        | `lint:rust` (`clippy`), `lint:errors` (型安全・エラー握りつぶし検査), `lint:toml`, `lint:ui` (`oxlint`) |
| `mise run types`                       | `fxg-protocol` から `specta` で `ui/src/lib/generated/bindings.ts` を再生成                             |
| `mise run sqlx:prepare` / `sqlx:check` | `crates/fxg-db/migrations/` を適用し `.sqlx` オフラインキャッシュを更新・検証                           |
| `mise run check:ui`                    | `svelte-check` による UI 型・テンプレート検査                                                           |
| `mise run test`                        | Rust ワークスペーステスト + UI (`Vitest`) を実行                                                        |
| `mise run check`                       | コミット前の全検証（`fmt:check` + `lint` + `check:ui` + `test`）                                        |
| `mise run build` / `build:docker`      | `fxg` リリースバイナリまたは Docker イメージをビルド                                                    |

---

## 2. 型生成・DB スキーマ変更時の手順

- **`crates/fxg-protocol` の型を変更した場合**: `mise run types` を実行し、`ui/src/lib/generated/bindings.ts` の差分をコミットする（生成物は直接編集しない）。
- **`crates/fxg-db/migrations/` を追加・変更した場合**: `mise run sqlx:prepare` を実行し、`.sqlx/` の更新をコミットする。
- **`sqlx::query!` 系マクロを追加・変更した場合**: 同じく `mise run sqlx:prepare` で `.sqlx/` を更新する。CI は欠落のみをコンパイル時に検出するため、古いメタデータのまま整合しているケースはローカルで防ぐ。
- **`ErrorCode` を追加・変更した場合**: `ui/messages/en.json` の対応キーと [`docs/troubleshooting.md`](./troubleshooting.md) の `## <ERROR_CODE>` セクションを追加する（`fxg-protocol` の契約テストで検証される）。

---

## 3. 言語ポリシーと i18n (`paraglide-js`)

- **コードは英語**: Rust / TypeScript / Svelte のコメント、doc コメント、ログ、CLI 出力、エラーメッセージ、UI 文言はすべて英語で記述する（多バイト境界テスト等の例外には `// allow-japanese: <理由>` を付記）。
- **ドキュメントは日本語**: `docs/` 配下（ステート docs・`proposals/`・`changelog/`）は日本語で維持する。
- **UI 多言語化**: `paraglide-js` を前提とし、Rust 側の API エラーは `ErrorCode` を i18n キーとして扱い、`message` は英語フォールバックとして返す（サーバー側でロケール交渉は行わない）。

---

## 4. CI (GitHub Actions)

CI は領域ごとに 3 つのワークフローへ分割している。目的は「変更していない領域のジョブを走らせない」こと。

- `.github/workflows/ci-fmt.yml`: 整形チェック (rustfmt / tombi / oxfmt)。`docs/` の Markdown も整形対象なので常時実行する。
- `.github/workflows/ci-rust.yml`: `crates/**`、TOML マニフェスト、`.sqlx/**`、生成 TS 型などの変更時に clippy / 3 OS テスト / specta 型同期を実行する。
- `.github/workflows/ci-ui.yml`: `ui/**` の変更時に oxlint / svelte-check / Vitest / ビルドを実行する。

不変条件:

- **draft PR では全ジョブをスキップ**し、`ready_for_review` で実行する。`pull_request` の `types` に `ready_for_review` を含めること（既定の 3 種類だけでは ready にした時点で走らない）。
- **`.sqlx` の検査は専用ジョブを持たない**。オフラインキャッシュの欠落は `sqlx::query!` のコンパイル時に失敗するため、clippy / テストが同じ検査を兼ねる。クエリやマイグレーションを変更したらローカルで `mise run sqlx:prepare`、検証のみなら `sqlx:check` を実行して `.sqlx/` を更新する。
- CI では debuginfo を `line-tables-only` に落とし、`Swatinem/rust-cache` の保存は `test` ジョブだけに限定する（同一キーへの並列保存はレースになり、immutable なキャッシュが上書きできない）。
- 将来 branch protection で required check を設定する場合、パスフィルタでスキップされたワークフローは Pending のまま残る。常時実行される集約ジョブを required にすること。
