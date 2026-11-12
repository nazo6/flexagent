# Agent モードを ACP Session Config Options から正規化する

- **日付**: 2026-11-12
- **対象パッケージ**: `fxg-acp`, `ui`
- **対象スクリプト**: `cargo test -p fxg-acp`, `pnpm --dir ui run check`, `pnpm --dir ui run test`

## 背景

OpenCode (v2.2.7) を ACP で接続すると、`plan` / `build` などのモードが UI に
表示されなかった。原因は ACP の仕様移行にある。

- ACP は Session Modes (`session/new` の `modes` / `session/set_mode`) を
  **Session Config Options** (`configOptions` の `category: "mode"` /
  `session/set_config_option`) に置き換えつつある。
  仕様は「config options に対応した Client は `modes` を無視すべき」としている。
- OpenCode は `modes` を返さず、`configOptions` に
  `{ id: "mode", category: "mode", currentValue: "build", options: [build, plan] }`
  だけを返す。移行期間中の「両方返す」推奨には従っていない。
- `fxg-acp` は `modes` しか読まず、Web UI も `availableModes` が空だとモード pill を
  出さない。さらに `category = "mode"` の configOption はモデル pill 列から
  除外していたため、モード情報が二重に落ちていた。
- 新規セッションの初期モードは `default / code / plan` 固定で、OpenCode に存在
  しない `code` を送ると `Invalid params: mode not found: code` になっていた。

## 変更内容

### `fxg-acp`

- `session_capabilities` が `(modes, current_mode, config_options)` を返すように
  なり、`category = "mode"` の `configOption` を `ModeInfo` /
  `current_mode` に投影する。両方ある場合は仕様どおり configOption を優先する。
- `mode_config_option` / `mode_state_from_config` を追加し、ACP 型から
  モード一覧 (名前・説明つき) を生成する。カテゴリ欠落時は `id == "mode"` を
  フォールバックに使う (ACP ではカテゴリは必須ではない)。
- `SessionCapabilitiesState::set_config_options` を追加。`config_option_update` と
  `session/set_config_option` の応答で `available_modes` / `current_mode` も
  同期する。更新は ACP の生オプションから直接投影するため、モード名・説明も
  維持される。
- `AcpSessionHandle::set_mode` は、モード用 configOption があれば
  `session/set_config_option`、無ければ `session/set_mode` を使う。
- 初期モードも同じ判定で適用し、`set_config_option` の応答を初期
  `configOptions` に反映する。
- `tests/fixtures/mock_agent.rs` に `modes-as-config` モードを追加
  (`session/set_mode` を拒否し、`configOptions` のみで build/plan を提示)。
- `tests/mode_config_option.rs` を追加し、初期モードとセッション中のモード変更が
  `session/set_config_option` 経由になることを検証する。

### `ui`

- 新規セッションの初期モード選択肢をプローブ結果の `available_modes` から生成する。
  モード ID はエージェント固有なので、プローブが答えるまでは `Default` のみを
  表示し、`code` / `plan` のような推測 ID は出さない。
- エージェントを変更したときは初期モードを `default` に戻す (モード ID は
  エージェントごとに異なるため)。プローブ結果に選択中のモードが無い場合も
  `default` に戻す。
- `SessionModePreference` を任意のモード ID を保持できる `string` に変更する
  (localStorage の既存値はそのまま読める)。

## 補足

- `docs/04-agent-drivers-and-windows.md` §2.2 / §4.6 を更新した。
- CLI / ハブは `CapabilitiesUpdated` をそのまま利用するため変更不要。
