# 05. CLI (`fxg`) UX・PWA フロントエンド設計・実装ロードマップ

---

## 1. `fxg` CLI コマンド体系

普段のターミナル作業で最小限のタイプ数で使えるよう、エイリアス的記法（`fxg <agent>`）を第一級サポートします。

```bash
# 1. エージェントの起動（カレントディレクトリで起動＆即座にターミナルAttach）
fxg opencode                   # opencode / opencode2 を起動 (純正TUI + リモート同期)
fxg antigravity                # ACP Registry の antigravity-acp を自動取得・起動 (内蔵TUI)
fxg claude                     # claude-code-acp を起動
fxg <agent> -- <extra-args>    # エージェント固有のCLI起動引数をパススルー

# 2. セッション一覧・再アタッチ
fxg ps                         # 現在稼働中・最近のセッション一覧を表示
fxg attach [session-id]        # 稼働中セッションにターミナルを接続（省略時はカレントフォルダの直近セッション）

# 3. プロジェクト・エージェント管理
fxg agents list                # ACP Registry の利用可能エージェント一覧とインストール状態
fxg agents install <id>        # ACP エージェントの事前ダウンロード
fxg project info               # カレントフォルダの論理プロジェクトID (Git正規化結果) を確認
fxg project link <project-id>  # 非Gitフォルダを特定のプロジェクトIDに手動紐付け

# 4. デーモン・サーバー・常駐サービス管理
fxg daemon                     # ノードデーモンをフォアグラウンド起動 (ローカルWeb UI: http://localhost:7860)
fxg service install            # OSログイン時の自動バックグラウンド起動を設定 (Win/Mac/Linux/WSL)
fxg service status             # デーモン稼働状態・中央サーバー接続状態・未同期Outbox件数を表示
fxg server --port 8080         # 中央サーバーを起動
```

---

## 2. Web UI / Android PWA (`ui/`) 設計

PCブラウザ、ローカルフォールバック (`localhost:7860`)、および Android
スマートフォン（ホーム画面追加PWA）のすべてを単一のレスポンシブSPAで提供します。

### 2.1 フロントエンド技術スタック

- **ビルド & フレームワーク**: Vite + React 19 + TypeScript
- **スタイリング**: Tailwind CSS
  (モバイルの片手操作・ボトムシートUIと、デスクトップのマルチペインUIをレスポンシブ切替)
- **状態管理 & 同期**: Zustand + カスタム WebSocket 差分同期ストア
  (`last_global_seq` 管理)
- **コード・Diff・ターミナル表示**:
  - Diff / コード表示: `@monaco-editor/react` (PC用) /
    軽量シンタックスハイライト `shiki` + Unified Diff ビューア
    (モバイル用高速描画)
  - ターミナル出力表示: `@xterm/xterm` (`TerminalOutput`
    イベントのANSIカラー出力をそのまま描画)
- **型安全性**: Rustの `fxg-protocol` から `ts-rs`
  で自動生成された型定義をインポート。

### 2.2 主要画面構成

1. **グローバル承認 Inbox 画面 (バッジ通知付き)**:
   - 全ノード・全セッションで現在 `waiting_permission`
     になっているリクエストを一箇所に集約。
   - 実行しようとしているコマンド（例:
     `cargo test`）や変更ファイルのDiffを確認し、ワンタップで **Approve / Allow
     Always / Reject** を応答。
2. **プロジェクト & セッション一覧画面**:
   - 論理プロジェクト（例: `github.com/nazo6/flexagent`）ごとにグループ化。
   - 「＋新規セッション」ボタンから、**実行ノード（Windows PC / WSL / VPS）** と
     **エージェント（opencode2 / antigravity-acp）** を選んでリモート起動。
   - 既存セッションから「別ノードへ履歴を引き継いでFork」するアクションもここから実行。
3. **セッション詳細（チャット & ワークスペース）画面**:
   - **ストリームタイムライン**:
     ユーザー発言、思考プロセス（折りたたみ可能）、ツール実行、ファイルDiff、ターミナル出力を表示。
   - **動的コントロールバー**:
     - `/` 入力時にACPの
       `AvailableCommand`（エージェント固有スラッシュコマンド）をサジェスト表示。
     - ACPの `SessionMode`（`plan` / `code` 等）および
       `ConfigOption`（モデル選択等）のドロップダウン切替。
     - 実行中ターンの `Cancel (中断)` ボタン、および実行中も次の指示を予約できる
       **Pending Queue（送信予約キュー）**。
4. **接続先スイッチャー（耐障害性サポート）**:
   - 通常は中央サーバーへ接続しますが、万が一中央サーバーがダウンしている場合は、画面上部のバナーから登録済みの各ノードのローカルWeb
     UI（例: `http://localhost:7860` や
     Tailscale上のノードURL）へワンタップで接続先を切り替えられます。

### 2.3 Android PWA & Web Push (VAPID) の実装詳細

- **`manifest.webmanifest`**: `"display": "standalone"`,
  `"theme_color": "#0f172a"`
  を設定し、Androidでネイティブアプリ同等のフルスクリーン起動を実現。
- **Service Worker (`sw.js`)**:
  1. **App Shell キャッシュ**:
     HTML/JS/CSSをキャッシュし、モバイル回線が不安定な場所や中央サーバー障害時でもUIが即座に立ち上がるようにします。
  2. **Web Push 受信 & アクションボタン**: 中央サーバーからVAPID Web
     Pushを受信した際、Android通知バナーに以下を表示します：
     - タイトル: `[flexagent] 承認リクエスト (Home-Win)`
     - 本文: `opencode2: Run "cargo test --workspace"`
     - アクションボタン: **`[Approve]`** / **`[Reject]`**
       ユーザーが通知バナー上の `[Approve]` をタップすると、Service
       Workerがバックグラウンドで
       `POST /api/v1/sessions/:id/permissions/:req_id/approve`
       を叩き、アプリ画面を開くことすらなく承認が完了します。

---

## 3. 段階的実装ロードマップ (Milestones)

手戻りを防ぎつつ、早い段階で実際に手元で動かせるようにする5つのフェーズです。

### Milestone 1: コアプロトコル・DB・ローカルデーモン基盤

- [ ] Cargo Workspace の構築 (`fxg-protocol`, `fxg-db`, `fxg-pty`, `fxg-acp`,
      `fxg-node`, `fxg-server`, `fxg-cli`)
- [ ] `node.db` / `server.db` のSQLiteマイグレーション実装
- [ ] Git Remote URL正規化による論理プロジェクト解決 (`fxg project info`)
- [ ] Windows Named Pipe / Unix Domain Socket による `fxg` CLI ⇔ `fxg daemon`
      ローカルIPC疎通

### Milestone 2: ACP ドライバ & Windows プロセス管理

- [ ] `fxg-pty`: ConPTY (`portable-pty`) + Windows Job Object (`win32job`) +
      `which` コマンド解決の実装
- [ ] `fxg-acp`: 公式 ACP Registry (`registry.json` / `agent.json`) のフェッチと
      `binary` / `npx` / `uvx` 起動実装
- [ ] `agent-client-protocol` クレートを用いた `AcpDriver` 実装（`fs/*`,
      `terminal/*`, `request_permission`, `AvailableCommand` 対応）
- [ ] `fxg <acp-agent>` でローカルCLI (TUI)
      からACPエージェントを対話実行・Attachできる状態にする

### Milestone 3: OpenCode2 ハイブリッド統合

- [ ] `OpenCode2Driver` の実装（`opencode2 serve`
      の起動・SSEイベント購読・OpenAPI操作）
- [ ] `fxg opencode` 実行時の純正TUI
      Attach（`opencode2 run --attach`）とデーモン側イベント記録の同時動作

### Milestone 4: 中央サーバー & Store-and-Forward 同期

- [ ] `fxg server` の Axum WebSocket Hub 実装
- [ ] `fxg daemon` の Outbox Sync Worker 実装（中央サーバーへのOutbound
      WS接続、切断時のローカル蓄積と再接続時の一括同期）
- [ ] 中央サーバー経由でのリモートコマンドルーティング（`StartSession`,
      `SendPrompt`, `RespondPermission`）

### Milestone 5: 共通 Web UI / Android PWA & Web Push

- [ ] `ui/` (Vite + React + TypeScript) の構築と `rust-embed` による `fxg`
      バイナリへの組み込み
- [ ] セッション画面（思考・Diff・xterm.jsターミナル・スラッシュコマンド補完・承認Inbox）の実装
- [ ] Service Worker (`sw.js`) と `web-push` クレートによる VAPID
      Push通知（Androidバックグラウンド通知＆バナー承認）の実装
- [ ] `fxg service install` による各OS自動起動設定の実装
