# Outbox のページングと ACP fs / バックグラウンドタスクの境界修正

- **日付**: 2026-11-17
- **対象パッケージ**: `fxg-db`, `fxg-node`, `fxg-protocol`, `fxg-agent`, `docs`
- **対象スクリプト**: `mise run sqlx:prepare`, `mise run fmt`,
  `cargo test --workspace --no-fail-fast`,
  `cargo clippy --workspace --all-targets`, `mise run lint:errors`

---

## 概要

リリースブロッカー対応 (#31) の「残り（別 PR 候補）」から、低リスクで
実害のある 2 点を実装した。Outbox の全件ロードをページングし、
バックグラウンドタスクの abort と ACP のファイル入出力を境界付きにした。

## 変更

### Outbox / Resync のページング

- `fxg-db` の Outbox 抽出を `(session_id, node_seq)` のキーセットカーソルで
  ページングする `extract_outbox_page` に置き換えた。従来は未送信イベントを
  全件メモリへ読み、送信メッセージも全件組み立ててから送っていたため、
  長期間オフラインだったノードの再接続時にメモリ使用量と初動が跳ねていた。
- `ResyncRequest` の再送（`extract_outbox_after`）も `from_node_seq` カーソルで
  ページングする。
- 水位（`synced_up_to_node_seq`）は ACK でのみ進むため、ページ単位の送信や
  途中切断で重複が生じても `INSERT OR IGNORE` と水位の `MAX` 更新で吸収される。
- 既定のページサイズは 1000 イベント（`EventBatchPush` は従来どおり 200 件
  ずつ）。

### `spawn_guarded!` / `spawn_logged!` の abort 伝播

- 内側の `tokio::spawn` を `Drop` で abort するガードで包み、返り値の
  `JoinHandle` を abort したときに内側の future も停止するようにした。
  従来は外側のラッパータスクだけが止まり、内側が動き続けていた。

### ACP `fs/read_text_file` / `fs/write_text_file`

- `std::fs` の同期呼び出しを `tokio::fs` に変更し、async ハンドラが
  ランタイムのワーカーを塞がないようにした。
- 読み書きするテキストは 8 MiB を上限とし、超過時はエラーを返す。
  既存ファイルが上限を超える場合の書き込みは、diff を省略して続行する。

## 検証

- `cargo test --workspace --no-fail-fast`
- `cargo test -p fxg-node --test sync_e2e outbox_paging_syncs_a_large_offline_backlog`
  （2500 件のモックバックログを実ハブへ同期し、ハブ側を消して再同期も確認）
- `cargo clippy --workspace --all-targets -- -D warnings`
- `mise run lint:errors`
- `mise run fmt:check`
- `mise run sqlx:prepare`
