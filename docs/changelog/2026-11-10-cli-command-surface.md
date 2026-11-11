# CLI コマンド体系の再編と node / hub 用語の統一

- **日付**: 2026-11-10
- **対象パッケージ**: `fxg-cli`, `fxg-protocol`, `fxg-db`, `fxg-node`,
  `fxg-hub`, `ui`
- **対象スクリプト / 設定**: `mise run fmt`, `mise run types`,
  `mise run sqlx:prepare`, `cargo check`, `cargo clippy`,
  `cargo test --workspace`

## 背景

CLI のサブコマンドが「セッション操作」「プロセスの前景起動」「OS 常駐登録」
「診断」「破壊的操作」を1つの階層に混在させており、コマンド名から責務が
読み取りにくかった。あわせて、同じコンポーネントを `node` / `daemon` /
`server` / `hub` / `central server` と呼び分けていた。

## 設計

- **セッション**は `fxg session ...` に集約する。
- **プロセス**は `node`（ローカル）と `hub`（中央）の2つだけにし、どちらも
  `start` / `stop` / `restart` / `status` を持つ。
- **`service`** は OS ログイン時の登録・解除だけを担当し、起動・停止は
  `node` / `hub` に委譲する。
- **診断**は `fxg doctor`（自己診断）と `fxg support bundle|incidents`
  （提出物・履歴）に分ける。
- **緊急停止**はハブの Web UI / `POST /api/v1/system/kill-switch` に置き、
  ローカルのノードは `fxg node stop` で停止する。
- 用語は `node` / `hub` に統一する。`daemon` / `server` / `central server`
  はコマンド名・設定名に使わない。

## コマンド体系

```text
fxg / fxg web [node|hub]
fxg session new|list|show|prompt|cancel|stop|kill|resume|fork|import|archive|unarchive|delete
fxg node start|stop|restart|status
fxg node access-token show|rotate
fxg hub start|stop|restart|status
fxg hub pairing-token issue|revoke|list
fxg service install|uninstall [node|hub]
fxg logs [node|hub] [--session <id>]
fxg doctor
fxg support bundle|incidents
fxg agents login|logout
fxg internal stdio|bootstrap-workspace|git-askpass
```

## 用語・保存先

| 項目         | 名前                                              |
| :----------- | :------------------------------------------------ |
| ローカル     | `node`（`fxg node start`、`node.db`、`node.log`） |
| 中央         | `hub`（`fxg hub start`、`hub.db`、`hub.log`）     |
| クレート     | `fxg-node` / `fxg-hub`                            |
| プロトコル   | `NodeToHubMsg` / `HubToNodeMsg`                   |
| config       | `[node]` / `[hub]`、`node.hub_url`                |
| OS 常駐 unit | `fxg-node` / `fxg-hub`                            |
| 秘密ストア   | `~/.flexagent/secrets/{node,hub}/<id>`            |

## 実装状況

本ブランチで CLI・プロトコル・DB・ハブ・UI を実装済み。
