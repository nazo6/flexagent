# OpenCode2 bridge のコンテキスト圧縮 (`/compact`) 対応

- **日付**: 2026-10-02
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `fxg-node`, `fxg-db`,
  `fxg-cli`,
  `ui`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run types`,
  `cargo test -p fxg-acp --lib`, `cargo clippy --workspace --all-targets`,
  `pnpm --dir ui run check`, `pnpm --dir ui run lint`, `pnpm --dir ui run test`

## 概要

`opencode2` ブリッジ (既定モード) でコンテキスト圧縮 (compaction)
を実行できるようにしました。`/compact` は opencode の TUI
がローカル処理するコマンドで `GET /api/command`
の一覧には現れませんが、専用 API
`POST /api/session/{id}/compact` が用意されており (opencode の ACP 実装も
`detectSlashCommand` の結果が `compact` の場合 `session.summarize`
へ特別振り分けする)、bridge からも同じ操作を提供します。

## 実装

| ファイル                                     | 内容                                                                                                                                                                      |
| :------------------------------------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `crates/fxg-protocol/src/common.rs`          | `CompactionStatus { Started, Completed, Failed }` を追加                                                                                                                  |
| 〃                                           | `SessionControlAction::Compact` を追加 (Web UI の「圧縮」ボタン用)                                                                                                        |
| `crates/fxg-protocol/src/events.rs`          | `UnifiedEventPayload::CompactionUpdated { status, detail }` を追加 (`event_type = "compaction_updated"`)                                                                  |
| `crates/fxg-acp/src/driver.rs`               | `ActiveSessionHandle::compact_context()` を追加 (既定は「未対応」エラー。ネイティブ API を持つドライバのみ実装)                                                           |
| `crates/fxg-acp/src/opencode2.rs`            | `compact_context()` = `POST /api/session/{id}/compact` (`{}` ボディ) を実装                                                                                               |
| 〃                                           | `send_prompt` で `/compact` (引数付きも許容、引数は無視) を専用 API へ振り分け。API 由来の同名コマンドが定義されている場合は従来の `/command` 送信を優先 (ACP と同じ順序) |
| 〃                                           | `fetch_capabilities` で `available_commands` に合成エントリ `compact` を追加 (同名のユーザー定義コマンドがある場合は追加しない)                                           |
| 〃                                           | SSE `session.compaction.started` / `.ended` を `CompactionUpdated` へ変換。要約本文の `session.compaction.delta` はエフェメラル扱いで破棄                                 |
| `crates/fxg-node/src/session_manager.rs`     | `ControlSession(Compact)` → `handle.compact_context()` を配線 (busy 中も許可 = steer 配送)                                                                                |
| `crates/fxg-db/src/events.rs`                | 投影対象外イベント群に `CompactionUpdated` を追加 (sessions 行の派生カラム更新なし)                                                                                       |
| `crates/fxg-db/src/searchable.rs`            | FTS 対象外イベント群に追加                                                                                                                                                |
| `crates/fxg-cli/src/tui.rs`                  | 開始/完了/失敗をシステム行として表示                                                                                                                                      |
| `crates/fxg-cli/src/commands.rs`             | `fxg session events` 等の表示に `compaction_updated <status>` を追加                                                                                                      |
| `ui/src/lib/sync/reducer.ts`                 | `compaction_updated` をタイムラインの notice 行へ変換 (started/completed = info、failed = error)。日本語ラベル付き                                                        |
| `ui/src/lib/components/chat/Composer.svelte` | capabilities に `compact` が含まれるときだけ表示する「圧縮」ボタンを追加。押下で `ControlSession(Compact)` を送信し、成功時に「圧縮をリクエストしました」をトースト       |

## 実機で確認した仕様 (opencode v2.0.21)

| エンドポイント / イベント             | 挙動                                                                                                                                                             |
| :------------------------------------ | :--------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `POST /api/session/{id}/compact`      | ボディ `{ id?, delivery? }` (省略可)。200 で `{ data: { type: "compaction", delivery: "steer", ... } }` を返す。busy 中は次のステップ境界で実行される steer 配送 |
| `session.compaction.started`          | `{ sessionID, reason: "manual"\|..., recent, inputID }`                                                                                                          |
| `session.compaction.delta`            | `{ sessionID, text }` (要約本文のストリーミング。実測 220 件)                                                                                                    |
| `session.compaction.ended`            | `{ sessionID, reason, model, text: <要約全文>, recent }`                                                                                                         |
| `session.execution.started/succeeded` | 圧縮も 1 ターンとして実行されるため、既存の `StatusChanged` で UI の実行中表示が自動更新される (専用イベント不要)                                                |
| `session.inbox.enqueued`              | `item.type = "compaction"` で発火するが `payload.text` を持たないため、既存ハンドラの空テキスト判定で安全に破棄される                                            |

## 設計判断

- **失敗イベントは成功/開始のみイベント化**: v2.0.21 には
  `session.compaction.failed` が無く、失敗時は `session.error`
  (既存の `DriverEvent::Failed`) として扱われる。`CompactionStatus::Failed`
  はプロトコルとしては用意し、UI/CLI は `detail` 付きで表示できる形にした。
- **delta は捨てる**: 要約本文は opencode 側の履歴 (compaction
  メッセージ) に残るため、fxg は開始/完了のみを永続化する。220
  件規模のチャンクをイベントログへ書かない。
- **引数は無視**: opencode の ACP と同じく `/compact <本文>` は引数を無視して
  圧縮のみ実行する (1 プロンプト 1 コマンドの原則)。
- **UI の能力判定は `available_commands`**: 専用の capability
  フラグを増やさず、「合成エントリ `compact` が一覧にあるか」で圧縮ボタンの
  表示を決める (プロトコル拡張を `SessionControlAction` のみに抑える)。
- **停止中セッションでは圧縮不可**: `ControlSession` は稼働中セッションのみを
  対象とし (UI も停止中はボタンを非表示)、停止中の `/compact`
  入力は送信経路の自動再開 (ネイティブ復元) 後に実行される。
- **ACP モードは変更なし**: 標準ACPに圧縮を開始する API
  は無く (`unstable_session_compaction` は圧縮*通知*の受信能力)、
  `AcpDriver` は `compact_context()` を実装しない。ACP モードでも手入力の
  `/compact` は opencode 側が `session/summarize`
  へ解釈するため従来どおり動作する (一覧・ボタンには出ない)。

## 制約 / フォローアップ

- legacy セッション (この対応前に起動したセッション) は capabilities の Replay
  に `compact` が無いため、再開 (ドライバ再起動)
  するまで圧縮ボタンは表示されない
  (`/compact` の手入力は可能)。
- v2.0.21 の SSE には `session.usage.updated` が存在する (実測)。
  `docs/04` の「opencode2 は usage を持たない」記述は将来更新が必要
  (取り込みは本対応の範囲外)。

## 検証

- `cargo test -p fxg-acp --lib` (`is_compact_command` / `with_compact_command` /
  compaction SSE 変換のユニットテスト)。
- `cargo test -p fxg-acp --lib -- --ignored compacts_context_against_real_opencode2`
  (実 opencode v2.0.21: 合成 `compact` の一覧、`/compact` 入力 →
  `session.compaction.started` / `.ended` の受信)。
- `cargo clippy --workspace --all-targets` / `pnpm --dir ui run check` /
  `pnpm --dir ui run lint` / `pnpm --dir ui run test` (reducer テスト追加)。
