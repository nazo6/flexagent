# FlexAgent (`fxg`) 設計・実装ドキュメント

**FlexAgent (`fxg`)**
は、コーディングエージェント（任意ACP対応エージェントおよび
`opencode2`）を「好きな場所から好きな場所で動かせる」セルフホスト型・ローカルファーストのエージェントマネージャです。

---

## コアコンセプト

1. **単一バイナリ (`fxg`) による完結**:
   - Rust製の単一バイナリ `fxg` に、**CLI (`fxg opencode`,
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

---

## ドキュメント構成

| ドキュメント                                                             | 内容                                                                                            |
| :----------------------------------------------------------------------- | :---------------------------------------------------------------------------------------------- |
| **[01-architecture-and-sync.md](./01-architecture-and-sync.md)**         | 全体トポロジー、ローカルファースト＆遅延同期（Outbox）プロトコル、論理プロジェクト同一性解決    |
| **[02-database-schema.md](./02-database-schema.md)**                     | 中央サーバー (`server.db`) とノードデーモン (`node.db`) のSQLiteスキーマ定義・FTS5検索設計      |
| **[03-protocol-and-api.md](./03-protocol-and-api.md)**                   | Rust共通型 (`fxg-protocol`)、Node⇔Server間WebSocket RPC、Client向けREST/WS API、ローカルIPC仕様 |
| **[04-agent-drivers-and-windows.md](./04-agent-drivers-and-windows.md)** | `AgentDriver` トレイト、ACP Registry自動解決、`opencode2` ハイブリッド統合、Windows固有実装     |
| **[05-cli-and-pwa-ui.md](./05-cli-and-pwa-ui.md)**                       | `fxg` CLIコマンド体系、PWA + Web Push (VAPID) フロントエンド設計、段階的実装ロードマップ        |

---

## リポジトリ・クレート構成 (Cargo Workspace)

```text
flexagent/
├── Cargo.toml                  # Rust Workspace定義
├── docs/                       # 設計・実装ドキュメント
├── crates/
│   ├── fxg-protocol/           # 共通型定義・ACP正規化イベント・WS/IPCメッセージ (ts-rs対応)
│   ├── fxg-db/                 # SQLiteスキーマ管理・クエリレイヤー (server.db / node.db 共用・分離)
│   ├── fxg-pty/                # ConPTY/Unix PTY・Windows Job Object・プロセスツリー管理
│   ├── fxg-acp/                # ACP Registry管理・AcpDriver・OpenCode2Driver実装
│   ├── fxg-node/               # ノードデーモン実装 (ローカルIPC・ローカルWeb配信・Outbox同期・Project解決)
│   ├── fxg-server/             # 中央サーバー実装 (Axum・ノード管理・イベント集約・Web Push・PWA配信)
│   └── fxg-cli/                # `fxg` バイナリエントリポイント (CLI / TUI / daemon / server サブコマンド)
└── ui/                         # 共通フロントエンド (Vite + React + TypeScript + Tailwind + Service Worker)
    ├── package.json
    ├── public/
    │   ├── manifest.webmanifest
    │   └── sw.js               # Web Push & オフラインApp Shellキャッシュ
    └── src/
```
