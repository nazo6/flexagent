# TypeScript 型生成の ts-rs → specta 移行

- **日付**: 2026-11-08
- **対象パッケージ**: `fxg-protocol` (依存追加: `specta` / `specta-serde` /
  `specta-typescript`)、`ui` (生成型の import 経路)
- **対象スクリプト / 設定**: `mise.toml` (`types`), `.cargo/config.toml`,
  `.github/workflows/ci.yml`, `Cargo.toml` (workspace deps), `AGENTS.md`,
  `docs/README.md`, `docs/03-protocol-and-api.md`, `docs/05-cli-and-pwa-ui.md`,
  `ui/src/lib/generated/README.md`

## 概要

`fxg-protocol` の TypeScript 型生成を **ts-rs** から
[**specta**](https://github.com/specta-rs/specta) へ移行した。

| 項目         | 移行前 (ts-rs)                                    | 移行後 (specta)                                       |
| :----------- | :------------------------------------------------ | :---------------------------------------------------- |
| 出力         | 型ごとに 1 ファイル (`ui/src/lib/generated/*.ts`) | 単一ファイル (`generated/bindings.ts`)                |
| ファイル数   | 102 (型 101 + `serde_json/JsonValue`)             | 1                                                     |
| import       | `$lib/generated/SessionSummary`                   | `$lib/generated/bindings`                             |
| 生成コマンド | `cargo test -p fxg-protocol export_bindings`      | 同左                                                  |
| 属性         | `#[derive(..., ts_rs::TS)]` + `#[ts(export)]`     | `#[derive(..., specta::Type)]` + `#[specta(collect)]` |

生成物は型名の昇順 (`Layout::FlatFile`) で安定出力されるため、型を 1
つ追加しても差分は 1 ファイルに収まる。

## 設計判断

- **`#[specta(collect)]` を採用**: `specta::collect()` が同一クレート内の
  収集対象型をまとめて `Types` に登録するため、ts-rs の `#[ts(export)]`
  と同じ運用 (型ごとに 1 属性) を維持できる。参照されている型は推移的に
  自動で取り込まれる。
- **エクスポータは lib のユニットテストに配置**
  (`crates/fxg-protocol/src/export_bindings.rs`): `collect` はリンカの
  セクション登録に依存するため、統合テスト (`tests/`) から呼ぶと
  ライブラリのオブジェクトがリンクされず **収集結果が空になる**。
  `#[cfg(test)] mod export_bindings;` として lib 内に置くことで確実に収集される。
- **`i64` / `u64` / `f64` は `number` として出力**: specta は精度損失
  (BigInt 系) と JSON で表現できない値 (`NaN` / `Infinity`) を避けるため、
  既定では `bigint` 禁止エラー / `number | null` になる。本プロトコルでは
  epoch ms・`node_seq`・コスト額など JSON 上は数値であり UI 側で変換を
  挟まないため、`export_bindings.rs` で `specta_typescript::Number` へ
  再帰的に置換している (`Types::map` + 自前の再帰変換。
  `Vec<T>` / `Map` は参照のインライン定義側にも実体があるため両方を辿る)。
- **`serde_json::Value` は `JsonValue` に改名**: UI 側の既存の型名
  (`JsonValue`) を維持するため、`module_path == "serde_json"` かつ
  `name == "Value"` の型をリネームする。
- **serde 属性は `specta-serde::Format` で反映**: `#[serde(tag = ...)]` /
  `rename_all` / `default` などをそのまま解釈する。結果として
  `#[serde(default)]` なフィールドが `?:` (省略可) になり、ts-rs より
  実態に近い型になる (`CustomAgentConfig` など)。

## 差分の例 (より正しい出力)

`#[serde(default)]` を持つ `CustomAgentConfig` は ts-rs では必須
プロパティだったが、specta では省略可能として出力される。

```ts
export type CustomAgentConfig = {
  name?: string | null,
  command?: string,
  args?: string[],
  env?: { [key in string]: string },
};
```

これにより `ui/src/lib/components/settings/AgentManager.svelte` の
`config?.args.join(" ")` が型エラーになったため
`(config?.args ?? []).join(" ")` に修正した (実装の誤りを型が検出した形)。

## 影響・移行手順

- UI の import 34 ファイル / 133 行を `$lib/generated/bindings` に集約。
- `.cargo/config.toml` の `TS_RS_EXPORT_DIR` / `TS_RS_LARGE_INT` を削除
  (出力先はエクスポータ側で固定)。
- `mise run types` は従来どおり `cargo test -p fxg-protocol export_bindings`。
  CI の `specta type sync` ジョブもそのまま `git diff --exit-code` で検証する。
- `ui/src/lib/generated/**` は oxfmt の `ignorePatterns` 対象のため、
  生成物は specta の出力そのまま (整形しない)。
