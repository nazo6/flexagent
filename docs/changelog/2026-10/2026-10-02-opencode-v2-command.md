# `opencode` コマンド名への追従と v2 判定の追加

- **日付**: 2026-10-02
- **対象パッケージ**: `fxg-acp`, `fxg-node`, `fxg-cli`, `fxg-protocol`
- **対象スクリプト / 設定**: `mise run fmt`, `cargo check --workspace`

## 概要

opencode v2 もコマンド名が `opencode` になった (v1 と同名) ため、起動する
実行ファイルを `opencode2` から `opencode` へ変更した。

ただし v1 の `opencode` は `serve` API・ネイティブセッション管理を持たず、
そのまま起動すると破綻する。そこで起動前に `opencode --version` を実行し、
**メジャーバージョンが 2 系でなければ起動しない**ようにした。

エージェントID (`opencode2`)・ドライバ種別 (`"opencode2"` / `"acp"`) は内部
識別子のため変更していない (設定・DB・プロトコルの互換を維持)。

## 実装

| ファイル                                 | 内容                                                                                                       |
| :--------------------------------------- | :--------------------------------------------------------------------------------------------------------- |
| `crates/fxg-acp/src/registry.rs`         | ビルトイン起動スペックの `program` を `opencode` に変更                                                    |
| `crates/fxg-acp/src/opencode2.rs`        | `ensure_opencode_v2` (`opencode --version` 検証) と `parse_opencode_major` を追加。テストの program も更新 |
| `crates/fxg-acp/src/lib.rs`              | `ensure_opencode_v2` を再エクスポート                                                                      |
| `crates/fxg-node/src/session_manager.rs` | `start_driver_session` で `opencode2` 起動前にバージョン検証 (bridge / acp 両モードをカバー)               |
| `crates/fxg-cli/src/tui.rs`              | 純正TUI Attach の実行コマンドを `opencode run ...` に変更                                                  |
| `crates/fxg-cli/src/commands.rs`         | `--acp` のヘルプ表記を `opencode acp` に更新                                                               |
| `crates/fxg-protocol/src/ipc.rs`         | `AttachMode::NativeOpenCodeAttach` のドキュメントコメントを更新                                            |
| `docs/04-agent-drivers-and-windows.md`   | コマンド例を `opencode serve` / `opencode run` / `opencode acp` に更新し、v2 判定を明記                    |

## 設計判断

- **判定は起動直前に一度だけ**: `SessionManager::start_driver_session` は
  start / resume / fork / ensure の全起動経路が通るため、ここで
  `spec.agent_id == OPENCODE2_ID` の場合のみ検証する (bridge / acp
  双方をカバー)。
- **メジャーバージョン一致で判定**: `opencode --version` の出力 (例:
  `opencode v2.0.21`) からメジャーを抽出し、`2` 以外は起動しない。
- **検証失敗の扱い**: ドライバ起動失敗と同様に `StatusChanged(Error)` を記録し、
  `NodeError::Agent` を返す。
