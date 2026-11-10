# CLI コマンド体系の再編と node / hub 用語の統一

- **日付**: 2026-11-10
- **対象パッケージ**: `fxg-cli`, `fxg-protocol`, `fxg-db`, `fxg-node`,
  `fxg-server` (→ `fxg-hub`), `ui`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run types`, `cargo check`,
  `cargo clippy`, `cargo test -p fxg-cli`

## 背景

CLI のサブコマンドが「セッション操作」「プロセスの前景起動」「OS 常駐登録」
「診断」「破壊的操作」を1つの階層に混在させており、次の点が分かりづらかった。

- `run` が何を起動するのか名前から読めない
- `daemon` / `server` / `service` の責務が重なり、`daemon` を止めるコマンドが無い
- `doctor` と `diagnostics` がどちらも「診断」を意味する
- `kill-all` がローカル限定なのに全ノード停止と読める（`--local-only` は no-op）
- `node` / `daemon` / `server` / `hub` / `central server` が同じものを指していた

## 決定事項

1. セッション操作は `fxg session ...` に集約し、`run` / `ps` は廃止する。
2. プロセスは `node`（ローカル）と `hub`（中央）の2つに統一し、どちらも
   `start` / `stop` / `restart` / `status` を持つ。
3. `fxg service` は OS ログイン時の登録・解除だけを担当し、起動・停止は
   `node` / `hub` に委譲する。
4. `kill-all` を廃止する。全ノード一斉の緊急停止はハブの Web UI /
   `POST /api/v1/system/kill-switch` に一本化し、ローカルは
   `fxg node stop` を使う。
5. 診断は `fxg doctor`（自己診断）と `fxg support bundle|incidents`
   （提出物・履歴）に分ける。
6. 用語は `node` / `hub` に統一する。`daemon` / `server` / `central server`
   はコマンド名・設定名に使わない。

## コマンド体系

```text
fxg / fxg web [node|hub]
fxg session new|list|show|prompt|cancel|stop|kill|resume|fork|import|archive|unarchive|delete
fxg node start|stop|restart|status
fxg node token show|rotate
fxg hub start|stop|restart|status
fxg hub node-token issue|revoke|list
fxg service install|uninstall [node|hub]
fxg logs [node|hub] [--session <id>]
fxg doctor
fxg support bundle|incidents
fxg agents login|logout
fxg internal stdio|bootstrap-workspace|git-askpass
```

## 用語・保存先の変更

| 変更前                                               | 変更後                                       |
| :--------------------------------------------------- | :------------------------------------------- |
| `fxg daemon`                                         | `fxg node start`                             |
| `fxg server`                                         | `fxg hub start`                              |
| `fxg run` / `fxg ps`                                 | `fxg session new` / `fxg session list`       |
| `fxg session stop --turn-only`                       | `fxg session cancel`                         |
| `fxg session diagnose`                               | `fxg session show` / `fxg logs --session`    |
| `fxg service start\|stop\|restart\|status`           | `fxg node\|hub start\|stop\|restart\|status` |
| `-d, --detach` (session new / fork / resume)         | `-q, --quiet`                                |
| `fxg auth token` / `rotate-token`                    | `fxg node token show` / `rotate`             |
| `fxg auth node-token`                                | `fxg hub node-token`                         |
| `fxg diagnostics export` / `incidents`               | `fxg support bundle` / `incidents`           |
| `fxg kill-all`                                       | 廃止（`fxg node stop` / ハブの kill switch） |
| `fxg daemon --stdio`                                 | `fxg internal stdio`                         |
| `server.db` / `server_settings`                      | `hub.db` / `hub_settings`                    |
| `[server]` / `node.central_server_url`               | `[hub]` / `node.hub_url`                     |
| `fxg-server` / `NodeToServerMsg` / `ServerToNodeMsg` | `fxg-hub` / `NodeToHubMsg` / `HubToNodeMsg`  |
| `--server` フラグ                                    | 位置引数 `node` / `hub`                      |
| `daemon.log` / `server.log`                          | `node.log` / `hub.log`                       |
| `fxg-daemon` / `fxg-server` (service unit)           | `fxg-node` / `fxg-hub`                       |

## 移行

- `server.db` が存在し `hub.db` が無い場合、起動時に `hub.db` へリネームする。
- `[server]` セクションは `[hub]` に改名する（旧セクションがあれば明示エラー）。
- 旧 `fxg daemon` / `fxg server` は隠しエイリアスを1リリースだけ残し、
  OS 常駐ユニットは `fxg service install` で書き換える。
- `run` / `ps` / `kill-all` / `diagnostics` / `auth` の互換エイリアスは
  作らない（0.1.0 のため）。

## 実装状況

本ファイルは設計の記録。CLI・プロトコル・DB・UI の実装は同じブランチで
後続して行う。
