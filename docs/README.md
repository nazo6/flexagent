# FlexAgent (`fxg`) 設計・実装ドキュメント

**FlexAgent (`fxg`)**
は、ACP（Agent Client
Protocol）対応コーディングエージェントを「好きな場所から好きな場所で動かせる」セルフホスト型・ローカルファーストのエージェントマネージャです。

---

## コアコンセプト

1. **単一バイナリ (`fxg`) による完結**:
   - Rust製の単一バイナリ `fxg` に、**CLI (`fxg session new` 等)**、**ノード
     (`fxg node`)**、**ハブ
     (`fxg hub`)**、および **Web UI / PWA (`rust-embed`)**
     をすべて内包します。
2. **ローカルファースト & 遅延同期 (Store-and-Forward)**:
   - 各ノード（PC / VPS / WSL）がローカルSQLite (`node.db`)
     とローカルWeb UI (`http://localhost:7860`。7860 が使用中の場合は空きポートへ
     フォールバックし、実際のアドレスは `node.addr` に記録)
     を持ち、ハブが停止していても100%自立稼働します。ハブ復旧時に未送信イベントを自動同期します。
3. **UIの完全統一 (Web / Desktop / Android PWA)**:
   - PCブラウザもAndroid端末も、単一の **PWA (Progressive Web App) + Web Push
     (VAPID)** で操作します。ネイティブアプリのビルド環境（Android SDK /
     Tauri等）やAPK更新作業は不要です。
4. **セルフホスト特化（E2E暗号化・マルチテナント省略）**:
   - 個人・単一テナントのセルフホストに特化することで、ハブのSQLite
     (FTS5) による全セッション横断検索や、軽量な差分同期を実現します。
5. **ファーストクラスのWindowsサポート**:
   - ConPTY、Windows Job Object によるプロセスツリー確実終了、`PATHEXT`
     解決、`dunce::canonicalize` によるパス正規化を備えます（WSL環境はWSL内にLinux版 `fxg`
     ノードを配置して別ノードとして統合）。
6. **LAN/VPN限定運用とブラウザ攻撃防御 (Defense in Depth)**:
   - ハブはLAN/プライベートVPN（Tailscale等）限定公開を前提とし、ノードは
     `127.0.0.1:7860`（既定。衝突時はループバックの空きポート）のみにバインド。
     暗号論的トークン認証、Host/Originヘッダ検証（DNS
     Rebinding/CSWSH対策）、リモートPTY制御、および緊急キルスイッチ（Panic
     Button）を標準装備します。
7. **ネットワーク非依存の一時VM・サンドボックスノード (`fxg internal stdio`)**:
   - 自前サーバー上の権限分離環境（Docker / Rootless Podman / Incus VM）や
     **Google Colab Pro (`google-colab-cli`)**
     などをオンデマンドにスポーンし、**標準入出力パイプ (`stdin/stdout`)
     上のJSON通信**で直結します。特定VPN（Tailscale等）へのロックインやポート開放を一切不要とし、環境セットアップはDockerfileやVM環境側で事前に用意することで高速・安全な完全ステートレス運用を実現します。

---

## ドキュメント構成

| ドキュメント                                                             | 内容                                                                                                                                     |
| :----------------------------------------------------------------------- | :--------------------------------------------------------------------------------------------------------------------------------------- |
| **[01-architecture-and-sync.md](./01-architecture-and-sync.md)**         | 全体トポロジー、ローカルファースト同期、一時VMノード (`fxg internal stdio` & 外部環境でのツール構築)、論理プロジェクト解決、セキュリティ |
| **[02-database-schema.md](./02-database-schema.md)**                     | `node.db` / `hub.db` 共通の単一SQLiteスキーマ（ロール差分・イベント投影）・FTS5検索・監査ログ                                            |
| **[03-protocol-and-api.md](./03-protocol-and-api.md)**                   | 共通型 (`fxg-protocol`)、Node⇔Hub間通信 (WS & Stdio)、Client API、ローカル HTTP/WS                                                       |
| **[04-agent-drivers-and-windows.md](./04-agent-drivers-and-windows.md)** | `AgentDriver` トレイト、エージェント実装の3層モデル、ACP Registry・PATH探索、ACPネイティブFork/Resume、Windows固有実装                   |
| **[05-cli-and-pwa-ui.md](./05-cli-and-pwa-ui.md)**                       | `fxg` CLIコマンド完全リファレンス、設定ファイルスキーマ (`config.toml`)、PWA + Web Push 設計                                             |
| **[06-type-safety.md](./06-type-safety.md)**                             | 型安全性ガイド (open enum / `RawJson` / 文字列判定の禁止 / `lint:typed` / 移行状況)                                                      |

---

## リポジトリ・クレート構成 (Cargo Workspace)

```text
flexagent/
├── Cargo.toml                  # Rust Workspace定義
├── mise.toml                   # ツール固定 (rust, node, pnpm) & 統合ビルド・チェックタスク定義
├── Dockerfile                  # コンテナイメージ (`mise run build:docker`)
├── compose.yaml                # ハブ (`hub`) / ノード (`node`) の compose 定義
├── docs/                       # 設計・実装ドキュメント
├── crates/
│   ├── fxg-protocol/           # 共通型定義・ACP正規化イベント・WS/Stdioメッセージ (specta対応)
│   ├── fxg-db/                 # SQLiteスキーマ管理・クエリレイヤー (hub.db / node.db 共用・分離)
│   ├── fxg-pty/                # ConPTY/Unix PTY・Windows Job Object・プロセスツリー管理
│   ├── fxg-acp/                # ACP Registry管理・AcpDriver・ローカル PATH コマンド解決
│   ├── fxg-node/               # ノード実装 (ローカルWeb/WS配信・Outbox同期・Project解決)
│   ├── fxg-hub/                # ハブ実装 (Axum・ノード管理・イベント集約・Web Push・PWA配信)
│   └── fxg-cli/                # `fxg` バイナリエントリポイント (CLI / node / hub サブコマンド・Web UI自動オープン)
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
