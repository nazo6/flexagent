# CI の高速化とワークフロー分割

- **日付**: 2026-11-15
- **対象パッケージ**: なし（GitHub Actions 設定）
- **対象スクリプト**: `mise run fmt:check`, `mise run lint:errors`, `mise run lint:toml`, `mise run test:rust`, `mise run types`, `mise run ui:*`

## 目的

CI が常に約 4〜6 分かかり、変更内容に関係なく全ジョブが走っていた。主な原因は次の 3 点。

- UI だけの PR でも Rust の 3 OS テストが走り、docs だけの PR でもフル CI が走っていた。
- `sqlx` ジョブが毎回 `sqlx-cli` をソースからビルドし (約 1 分 45 秒)、さらに
  `cargo sqlx prepare --check` (約 1 分 40 秒) を実行していた。しかし `.sqlx` の
  欠落は `sqlx::query!` のコンパイル時に検出されるため、検査が重複していた。
- `Swatinem/rust-cache` のキーが `ubuntu` / `ubuntu-latest` の 2 系統に分かれ、
  同一キーへの並列保存レースと 10 GiB のキャッシュ上限の圧迫が起きていた。

## 変更内容

### ワークフロー分割と実行条件

- `ci.yml` を 3 つに分割した。
  - `ci-fmt.yml`: 常時実行（`docs/**` の Markdown も整形対象のため）。
  - `ci-rust.yml`: `crates/**`、`**/*.toml`、`Cargo.lock`、`.sqlx/**`、
    `ui/src/lib/generated/**` の変更時。
  - `ci-ui.yml`: `ui/**`、`mise.toml` の変更時。
- draft PR では全ジョブをスキップし、`ready_for_review` で実行する。
  `pull_request` の `types` に `ready_for_review` を追加した。

### Rust ジョブの短縮

- `sqlx` ジョブを削除した。`.sqlx` の検証はコンパイル時のクエリマクロに任せ、
  ローカルでの更新・検証は従来どおり `mise run sqlx:prepare` / `sqlx:check` を使う。
- clippy は `lint:errors` のみ実行する。`lint:errors` は `-D warnings` を含む
  `lint:rust` の上位互換で、2 回目の clippy パスは不要だった。
- CI の debuginfo を `line-tables-only` にした (`CARGO_PROFILE_DEV_DEBUG` /
  `CARGO_PROFILE_TEST_DEBUG`)。backtrace の行番号は残しつつ、Windows の PDB 生成と
  リンク時間を削減する。
- `rust-cache` のキーを OS 単位 (`shared-key: workspace`) に統一し、保存は `test`
  ジョブだけに限定した。`cache-on-failure: true` で失敗時もキャッシュを残す。
  debuginfo の変更は既にキャッシュキーの `CARGO_*` ハッシュに含まれる。

## 検証

- `actionlint` で 3 ワークフローの構文・式・`paths` を検証（エラーなし）。
- 変更前の実測（warm cache）: 合計 4:01（windows test 3:57、sqlx 3:39、clippy 2:19）。
- 変更後の実測（run `43609247600`）:
  - 初回はキャッシュキー変更のため cold で約 7:20（macOS test 7:12 が支配的）。
  - 2 回目（warm）は合計 3:34。

| ジョブ         | 変更前 | 変更後 (warm) |
| :------------- | -----: | ------------: |
| test (windows) |   3:57 |          3:14 |
| test (macos)   |   3:30 |          3:33 |
| test (ubuntu)  |   1:55 |          1:53 |
| lint (clippy)  |   2:19 |          1:51 |
| specta types   |   0:57 |          0:28 |
| **合計**       |   4:01 |      **3:34** |

- キャッシュは OS あたり 1 エントリ（Linux 503MB / Windows 420MB / macOS 412MB）に
  なった。macOS は復元に 42 秒かかっており、これが次のボトルネック。
- 残課題: macOS の `test:rust`（155 秒）は未変更ワークスペースクレートの再コンパイルが
  支配的なため、`sccache`（GHA バックエンド）の導入でさらに短縮できる見込み。
