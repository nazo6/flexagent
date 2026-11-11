# config.toml のスキーマ厳格化（未知キーの拒否）

- **日付**: 2026-11-11
- **対象パッケージ**: `fxg-protocol`, `fxg-cli`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run lint:errors`,
  `cargo test --workspace`
- **関連ドキュメント**:
  - [`docs/05-cli-and-pwa-ui.md`](../05-cli-and-pwa-ui.md) §2.2
  - [`2026-11-11-node-pairing-db-and-gui.md`](2026-11-11-node-pairing-db-and-gui.md)

## 背景

`config.toml` のパースは `#[serde(default)]` のみで、未知のキーを黙って無視していました
（`unknown_fields_are_ignored_for_forward_compatibility` というテストで固定）。
この挙動には次の問題があります。

- キーのタイポ（`listen_adrr` など）が既定値へのフォールバックとして扱われ、
  起動は成功するが意図した設定が効かない。
- ペアリングの DB 移行で `[node] hub_url` が不要になったが、無視されるため
  ユーザーが古い設定に気付けない（前 PR では生の TOML を別途読む
  `legacy_hub_url()` で警告していた）。

プリリリースで後方互換を維持する必要がないため、設定ファイルを **型に厳格** に
一致させる方針にしました。

## 変更

- `GlobalConfig` / `NodeBootstrapConfig` / `HubBootstrapConfig` / `LogConfig` に
  `#[serde(default, deny_unknown_fields)]` を付与。未知のキーは
  `ConfigError::Parse`（`failed to parse config file ...: unknown field ...`）として
  起動時に拒否します。
- セクション自体の省略は引き続き既定値で補います（`default` は維持）。
- `legacy_hub_url()` と、それに依存していた `fxg node start` の警告・
  `fxg doctor` の専用チェックを削除。`[node] hub_url` は他の未知キーと同じ
  「解析エラー」として報告されます。
- テストを反転: `unknown_fields_are_rejected`（タイポ）と
  `leftover_hub_url_is_rejected`（廃止キー）を追加。

## 影響（破壊的変更）

- 既存の `config.toml` に廃止済みのキー（`[node] hub_url`、`[agents]`、
  `[provisioners]` など）やタイポが残っている場合、`fxg node start` / `fxg hub start`
  は起動に失敗します。エラーメッセージに該当キー名が出るので、その行を削除して
  ください。移行コマンドは用意しません（プリリリースのため）。
- 環境変数（`FXG_*`）は従来どおりキー単位で読み、未知の環境変数は無視されます
  （プロセスの環境全体を検証するものではない）。

## 検証

- `cargo test -p fxg-protocol`（未知キー・`hub_url` の拒否テスト）
- `mise run lint:errors`, `cargo test --workspace`
- 手動: `[node] hub_url` を残した `config.toml` で `fxg node start` が
  `unknown field \`hub_url\`` で失敗すること。
