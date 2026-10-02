# FlexAgent (`fxg`) 設計・実装ドキュメント

**FlexAgent (`fxg`)**
は、コーディングエージェント（任意ACP対応エージェントおよび
`opencode2`）を「好きな場所から好きな場所で動かせる」セルフホスト型・ローカルファーストのエージェントマネージャです。

---

## コアコンセプト

1. **単一バイナリ (`fxg`) による完結**:
   - Rust製の単一バイナリ `fxg` に、**CLI (`fxg run`,
     `fxg attach`)**、**ノードデーモン (`fxg daemon`)**、**中央サーバー
     (`fxg server`)**、および **Web UI / PWA (`rust-embed`)**
     をすべて内包します。
2. **ローカルファースト & 遅延同期 (Store-and-Forward)**:
   - 各ノード（PC / VPS / WSL）のデーモンがローカルSQLite (`node.db`)
     とローカルWeb UI (`http://localhost:7860`)
     を持ち、中央サーバーが停止していても100%自立稼働します。中央サーバー復旧時に未送信イベントを自動同期します。
3. **UIの完全統一 (Web / Desktop / Android PWA)**:
   - PCブラウザもAndroid端末も、単一の **PWA (Progressive Web App) + Web Push
     (VAPID)** で操作します。ネイティブアプリのビルド環境（Android SDK /
     Tauri等）やAPK更新作業は不要です。
4. **セルフホスト特化（E2E暗号化・マルチテナント省略）**:
   - 個人・単一テナントのセルフホストに特化することで、中央サーバーのSQLite
     (FTS5) による全セッション横断検索や、軽量な差分同期を実現します。
5. **ファーストクラスのWindowsサポート**:
   - `tmux` に依存しない Named Pipe マルチプレクシング、ConPTY、Windows Job
     Object によるプロセスツリー確実終了、`PATHEXT`
     解決を備えます（WSL環境はWSL内にLinux版 `fxg`
     デーモンを配置して別ノードとして統合）。
6. **LAN/VPN限定運用とブラウザ攻撃防御 (Defense in Depth)**:
   - 中央サーバーはLAN/プライベートVPN（Tailscale等）限定公開を前提とし、ノードデーモンは
     `127.0.0.1`
     のみにバインド。暗号論的トークン認証、Host/Originヘッダ検証（DNS
     Rebinding/CSWSH対策）、リモートPTY制御、および緊急キルスイッチ（Panic
     Button）を標準装備します。
7. **ネットワーク非依存の一時VM・サンドボックスノード (`fxg daemon --stdio`)**:
   - 自前サーバー上の権限分離環境（Docker / Rootless Podman / Incus VM）や
     **Google Colab Pro (`google-colab-cli`)**
     などをオンデマンドにスポーンし、**標準入出力パイプ (`stdin/stdout`)
     上のJSON通信**で直結します。特定VPN（Tailscale等）へのロックインやポート開放を一切不要とし、`mise`
     / `uv`
     による完全ステートレスな自動ツール構築と破棄前のGit成果物退避を備えます。

---

## ドキュメント構成

| ドキュメント                                                             | 内容                                                                                                                  |
| :----------------------------------------------------------------------- | :-------------------------------------------------------------------------------------------------------------------- |
| **[01-architecture-and-sync.md](./01-architecture-and-sync.md)**         | 全体トポロジー、ローカルファースト同期、一時VMノード (`--stdio` & 自動ツール構築)、論理プロジェクト解決、セキュリティ |
| **[02-database-schema.md](./02-database-schema.md)**                     | `node.db` / `server.db` 共通の単一SQLiteスキーマ（ロール差分・イベント投影）・FTS5検索・監査ログ                      |
| **[03-protocol-and-api.md](./03-protocol-and-api.md)**                   | 共通型 (`fxg-protocol`)、Node⇔Server間通信 (WS & Stdio)、Git認証プロキシ/Bundle退避、Client API、ローカルIPC          |
| **[04-agent-drivers-and-windows.md](./04-agent-drivers-and-windows.md)** | `AgentDriver` トレイト、ACP Registry自動解決、`opencode2` ハイブリッド統合、Windows固有実装                           |
| **[05-cli-and-pwa-ui.md](./05-cli-and-pwa-ui.md)**                       | `fxg` CLIコマンド完全リファレンス、設定ファイルスキーマ (`config.toml` / `.fxg.toml`)、PWA + Web Push 設計            |

---

## リポジトリ・クレート構成 (Cargo Workspace)

```text
flexagent/
├── Cargo.toml                  # Rust Workspace定義
├── mise.toml                   # ツール固定 (rust, node, pnpm) & 統合ビルド・チェックタスク定義
├── Dockerfile                  # コンテナイメージ (`mise run build:docker`)
├── compose.yaml                # 中央サーバー (`server`) / ノード (`node`) の compose 定義
├── docs/                       # 設計・実装ドキュメント
├── crates/
│   ├── fxg-protocol/           # 共通型定義・ACP正規化イベント・WS/IPCメッセージ (ts-rs対応)
│   ├── fxg-db/                 # SQLiteスキーマ管理・クエリレイヤー (server.db / node.db 共用・分離)
│   ├── fxg-pty/                # ConPTY/Unix PTY・Windows Job Object・プロセスツリー管理
│   ├── fxg-acp/                # ACP Registry管理・AcpDriver・OpenCode2Driver実装
│   ├── fxg-node/               # ノードデーモン実装 (ローカルIPC・ローカルWeb配信・Outbox同期・Project解決)
│   ├── fxg-server/             # 中央サーバー実装 (Axum・ノード管理・イベント集約・Web Push・PWA配信)
│   └── fxg-cli/                # `fxg` バイナリエントリポイント (CLI / TUI / daemon / server サブコマンド)
└── ui/                         # 共通フロントエンド (SvelteKit / Svelte 5 SPA + TypeScript + Tailwind v4 + shadcn-svelte)
    ├── package.json            # パッケージ管理: pnpm / 品質管理: oxlint, oxfmt, svelte-check, Vitest
    ├── pnpm-lock.yaml
    ├── static/
    │   └── manifest.webmanifest
    └── src/
        ├── service-worker.ts   # Web Push & オフラインApp Shellキャッシュ ($service-worker)
        ├── lib/
        └── routes/
```
