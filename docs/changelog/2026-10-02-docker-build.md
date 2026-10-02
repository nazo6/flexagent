# Docker イメージビルド (`mise run build:docker`) と docker compose の追加

- **日付**: 2026-10-02
- **対象パッケージ**: ワークスペース全体 (`fxg-cli` / `fxg-ui-assets` / `ui`)
- **対象スクリプト / 設定**: `Dockerfile`, `compose.yaml`, `.dockerignore`,
  `.gitignore`, `mise.toml` (`build:docker` タスク)

## 概要

単一バイナリ `fxg` (CLI / TUI、ノードデーモン、中央サーバー、PWA) を内包した
Docker イメージを `mise run build:docker` (= `docker compose build`)
でビルドできるようにした。あわせて、中央サーバー (`fxg server`) とノード
(`fxg daemon`) を起動する `compose.yaml` を追加した。

## 実装

| ファイル        | 内容                                                                                                       |
| :-------------- | :--------------------------------------------------------------------------------------------------------- |
| `Dockerfile`    | 3 ステージ構成 (`ui` → `build` → `runtime`)                                                                |
| `compose.yaml`  | `server` (`fxg server`) / `node` (`fxg daemon`) の compose 定義 (共通定義は `x-fxg` アンカーで共有)        |
| `.dockerignore` | `target/` / `ui/node_modules` / `ui/build` / `.git` / `workspace` / 開発用 DB をビルドコンテキストから除外 |
| `.gitignore`    | `compose.override.yaml` / `.env` / `workspace/` を追加                                                     |
| `mise.toml`     | `build:docker` タスク (`docker compose build`) を追加                                                      |

- **Stage 1 (`ui`)**: `node:24-bookworm-slim` + `pnpm@12.8.1` (mise.toml
  と同一ピン)
  で `ui/build` を生成する。
- **Stage 2 (`build`)**: `rust:1.94-slim-bookworm` (MSRV = `rust-version` 1.94)
  で
  `cargo build --release --locked --package fxg-cli`。`.sqlx/` を使うため
  `SQLX_OFFLINE=true` で DB 接続不要。`ui/build` (rust-embed) と
  `ui/static/icon-192.png` (`tray/mod.rs` の `include_bytes!`) を Stage 1
  からコピーする。
  `registry` / `target` は BuildKit キャッシュマウントで再利用する。
- **Stage 3 (`runtime`)**: `debian:bookworm-slim` + `ca-certificates` / `git` /
  `openssh-client`。`FXG_HOME=/data` (VOLUME) に `fxg` ユーザー (UID 1000)
  で実行し、
  既定コマンドは `fxg server` (`EXPOSE 8080`)。

## 設計判断

- **実行イメージの最小化**: UI は `rust-embed` でバイナリに同梱されるため
  Node.js を
  含めない。SQLite は bundled 静的リンク、TLS は rustls のため `libssl` も不要。
  git は `fxg daemon` の Worktree / Shadow Git Tree
  が外部コマンドとして要求する。
- **データ永続化**: `FXG_HOME=/data` に固定して `VOLUME` を宣言し、`server.db` /
  `node.db` / `config.toml` / `worktrees` を named volume で保持する。

## docker compose (`compose.yaml`)

`server` (中央サーバー) と `node` (`fxg daemon`) の 2 サービス構成。共通定義
(`platform` / `build` / `image` / `init` / `restart`) はトップレベルの `x-fxg`
アンカーに集約して両サービスで使い回す。同じイメージ (単一バイナリのサブコマンド
違い) を使うため、レジストリのイメージへ差し替える場合は `compose.override.yaml`
などで **両サービス** に `image` を指定する。

### `server` (中央サーバー)

| 設定                               | 内容                                                                                         |
| :--------------------------------- | :------------------------------------------------------------------------------------------- |
| `init: true`                       | `fxg server` は子プロセスを spawn するため、PID 1 のゾンビ回収を docker-init (tini) に任せる |
| `restart: unless-stopped`          | サーバー再起動後も自動復帰する                                                               |
| `ports: "8080:8080"`               | ループバック限定にする場合は `"127.0.0.1:8080:8080"`                                         |
| `volumes: fxg-data:/data`          | `FXG_HOME` 配下 (`server.db` / `config.toml` / `logs` 等) を永続化                           |
| `FXG_SERVER_ALLOWED_HOSTS/ORIGINS` | LAN / Tailscale のホスト名・IP でアクセスする場合のみ設定 (コメントで例を記載)               |

### `node` (ノードデーモン)

`command: ["daemon"]` で `fxg daemon` として起動し、ローカル Web UI
とエージェント
実行を担う。ホストのリポジトリを `/workspace` に bind mount して操作させる。

| 設定                                       | 内容                                                                                               |
| :----------------------------------------- | :------------------------------------------------------------------------------------------------- |
| `command: ["daemon"]`                      | イメージの既定 (`fxg server`) を `fxg daemon` に差し替える                                         |
| `depends_on: server`                       | 起動順のみサーバーを先にする (サーバー停止中でもノードはローカルで自立稼働する)                    |
| `ports: 127.0.0.1:7860`                    | ノードのローカル Web UI。Host 検証がループバック固定のためホストのループバックへ公開する           |
| `FXG_NODE_LISTEN_ADDR`                     | コンテナ内の待受を `0.0.0.0:7860` にする (公開先はホストのループバックのみ)                        |
| `FXG_NODE_ID` / `FXG_NODE_NAME`            | `docker` 固定 (未指定時はコンテナ ID ベースの自動採番になるため明示)                               |
| `FXG_CENTRAL_SERVER_URL`                   | `ws://server:8080/api/v1/node/ws` (compose ネットワーク内でサーバーへ接続)                         |
| `FXG_NODE_TOKEN`                           | `.env` の `FXG_NODE_TOKEN` を渡す。空の場合はローカル専用ノードとして動作 (同期は無効)             |
| `volumes: fxg-node-data:/data`             | `node.db` / `auth_token` / `worktrees` / `snapshots` 等を永続化                                    |
| `${FXG_WORKSPACE:-./workspace}:/workspace` | 操作対象のリポジトリ群。起動後に `docker compose exec node fxg project scan /workspace` で登録する |

ペアリングトークンはサーバー上で
`docker compose exec server fxg auth node-token issue docker` を実行して発行し、
`.env` (`FXG_NODE_TOKEN=...`) に保存する。`FXG_ALLOW_REMOTE_PTY: "true"`
を設定すると
中央サーバー (Web UI / PWA) 経由の対話型 PTY を許可する。
