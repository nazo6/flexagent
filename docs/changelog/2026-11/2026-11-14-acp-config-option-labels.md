# ACP config option の表示名・説明を保持する

- **日付**: 2026-11-14
- **状態**: 実装済み
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `ui`, `docs`
- **対象スクリプト**: `mise run types`, `cargo test -p fxg-protocol -p fxg-acp`,
  `cargo clippy -p fxg-protocol -p fxg-acp --all-targets`,
  `pnpm --dir ui run test`, `pnpm --dir ui run check`, `mise run fmt`

---

## 1. 背景

ACP の `session/new` / `config_option_update` は、select の選択肢を
`{ value, name, description }` で返す。しかし fxg は `ConfigOptionValue::Select`
（文字列）に `value` だけを残していたため、次の情報が失われていた。

- `agy` のモデル名 (`gemini-4-argon` → `Gemini 4 Argon`)
- `agy` の Thinking Level の表示名 (`low` → `Low`)
- モード選択肢の説明
- 将来のエージェントが返すモデル説明

OpenCode は Tier 2 extension でモデル名を再生成して補っていたが、ブランド名の
ハードコードを維持する必要があり、agy など Tier 1 のエージェントには効かなかった。

## 2. 変更内容

- `fxg-protocol`:
  - `ConfigOptionChoice { value, name, description }` を追加した。
  - `ConfigOptionValue` に `Choice(ConfigOptionChoice)` を追加した。
    `#[serde(untagged)]` の既存形状（文字列 / 真偽値 / 任意 JSON）は維持する。
  - `ConfigOptionChoice::from_value()` と
    `ConfigOptionValue::select_value()` を追加し、ワイヤ値だけを持つ内部値も
    同じ経路で扱えるようにした。
- `fxg-acp`:
  - `map_config_option` が ACP の `SessionConfigSelectOption` を `Choice` に
    変換する。`current_value` は options から同じ値を引いて name / description を
    保持する。
  - `parse_config_value` は `Choice.value` だけを `session/set_config_option`
    に送る。
  - モード変更後に `current_value` を差し替えるときも、options にある
    `Choice` を再利用する。
  - `OpenCodeExtension` からモデル名の再生成 (`transform_model_value` /
    `display_model_name` / `title_token`) を削除した。ACP の name をそのまま
    使う。
- `ui`:
  - `choiceLabel` は `name` を使い、`value` が `provider/model` で name が
    同じ provider prefix を持つ場合は primary label から prefix を除く。
  - `configChoiceDescription` は `description` を優先し、無ければ model の
    provider を返す。
  - `configValueKey` は `Choice.value` を同一性のキーにし、表示名だけが
    変わっても保存済みのモデル設定と favorites を維持する。
  - `resolveInitialConfig` は保存値が見つかった場合、現在の選択肢の `raw`
    （新しい `Choice`）を返す。

## 3. データ互換性

- 未デプロイのため旧データの読み取り互換は持たない。`ConfigOptionValue` から
  `Select(String)` を削除し、select は常に `Choice`
  (`{ value, name, description }`) で表す。既存の dev DB /
  localStorage は破棄する前提。
- `Unknown` は将来のエージェント固有値のための型付き逃げ道として残すが、
  旧 OpenCode extension の `{ providerID, id, name }` は特別扱いしない。
- `set_config` の wire 形式は `{ value: "..." }` のままで、ACP 側の解釈は
  変わらない。

## 4. テスト

- `fxg-acp`:
  - grouped select の `current_value` / options が `Choice` になること。
  - `Choice` が `session/set_config_option` の wire value に変換されること。
  - モードの初期値が options の `Choice` を保持すること。
- `ui`:
  - ACP の name / provider prefix の表示。
  - `description` と provider の secondary label。
  - `Choice` の wire value による同一視。
  - 保存済みモデルの引き継ぎと favorites key。
- `cargo test -p fxg-protocol -p fxg-acp` / `pnpm --dir ui run test` /
  `pnpm --dir ui run check`。
