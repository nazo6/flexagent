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
- **コード・Diff・双方向ターミナル**:
  - **Diff / コード表示**:
    - PC: `@monaco-editor/react` (Monaco Diff Editor: Side-by-side / Inline
      切替、ミニマップ、構文ハイライト)
    - モバイル: 軽量シンタックスハイライト `shiki` + Unified Diff ビューア
      (折りたたみ・変更行ハイライト)
  - **双方向ターミナル**:
    - **コアエンジン**: `ghostty-web` (WebAssembly版 `libghostty-vt` + Canvas 2D
      レンダラ)
    - **抽象化レイヤー (`ITerminalAdapter`)**:
      将来的なレンダラ差し替え（DOMベースの `wterm`
      など）やテスト容易性を担保する薄いラッパー設計。
    - ノードの ConPTY / Unix PTY と WebSocket (`/api/v1/pty/ws`)
      で直結し、キー入力・リサイズ・ANSIカラー出力を双方向ストリーミング。
    - モバイルPWA向け: 画面下部に `Ctrl`, `Esc`, `Tab`, `↑`, `↓`, `←`, `→`
      などの仮想キーバーを提供。
- **型安全性**: Rustの `fxg-protocol` から `ts-rs`
  で自動生成された型定義をインポート。

### 2.2 主要画面構成

1. **グローバル承認 Inbox 画面 (バッジ通知付き)**:
   - 全ノード・全セッションで現在 `waiting_permission`
     になっているリクエストを一箇所に集約。
   - コマンド実行（例:
     `cargo test`）やファイル変更のDiffをその場でプレビューし、ワンタップで
     **Approve / Allow Always / Reject** を応答。
2. **プロジェクト & Worktree 一覧画面**:
   - 論理プロジェクト（例: `github.com/nazo6/flexagent`）ごとにグループ化。
   - 各ノード上の **Git Worktree
     一覧**（メインリポジトリ、各ブランチ、未コミット差分件数）を可視化。
   - 「＋新規セッション」ボタンから、既存のWorktreeを選択、または
     **「新規Worktree（ブランチ名指定）を作成して起動」** を実行。
   - 既存セッションの別ノードへのContext
     Forkや、不要になったWorktreeの削除もここから実行。
3. **セッション詳細（チャット & ワークスペース）画面**:
   - **ヘッダー**: 実行ノード名（例:
     `Home-Win`）、接続状態、バインドされているWorktree（例:
     `feat/auth`）を常時表示。
   - **マルチペイン / タブ構成** (デスクトップは左右分割、モバイルはタブ切替):
     - **Chat ペイン**:
       ストリームタイムライン（ユーザー発言、思考プロセス折りたたみ、ツール実行、承認カード、Pending
       Queue）。
     - **Diff ペイン (2段階スコープ切替)**:
       - **「セッションの変更 (This Session)」**:
         このセッションでエージェントが編集したファイル一覧とDiff（ノードがオフラインでも中央サーバーから100%閲覧可能）。
       - **「ノードのGit作業ツリー (Worktree Diff)」**:
         ノードにリアルタイム問い合わせる最新差分。
         - `(●) vs Base (main...HEAD)`:
           ベースブランチとの累積差分（PRレビュー感覚で全体の変更を把握）。
         - `( ) vs HEAD (Uncommitted)`: 作業ツリーの未コミット差分。
         - ファイルツリー（変更行数バッジ `+45 -2`）からファイルを選択して
           Monaco Diff Editor で閲覧。
     - **Terminal ペイン (統合 Web PTY)**:
       - 画面下部ドロワー（開閉可能）または独立タブで展開。
       - セッションの作業ディレクトリ（Worktree）上で動作するシェル（PowerShell
         / bash / zsh）を直接対話操作。
       - エージェントが実行している対話型コマンドへのキー入力送信（`TerminalInput`）にもシームレスに切り替え可能。
   - **動的コントロールバー**:
     - スラッシュコマンド補完、モード切替 (`plan` /
       `code`)、モデル選択、中断ボタン。
4. **接続先スイッチャー（耐障害性サポート）**:
   - 通常は中央サーバーへ接続しますが、万が一中央サーバーがダウンしている場合は、画面上部のバナーから登録済みの各ノードのローカルWeb
     UI（`http://localhost:7860`）へワンタップで接続先を切り替え。

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

### 2.4 統合 Web ターミナルと抽象化設計 (`ITerminalAdapter`)

Webターミナルは、GhosttyのネイティブVTエミュレーション精度とCanvas
2D高速描画を持つ **`ghostty-web`**
を標準採用します。同時に、UIコンポーネントと特定のターミナル実装を疎結合にするため、薄い抽象化インターフェースを介して利用します。

```typescript
// ui/src/components/terminal/types.ts
export interface TerminalDimensions {
  cols: number;
  rows: number;
}

export interface ITerminalAdapter {
  mount(element: HTMLElement): void;
  write(data: string | Uint8Array): void;
  onData(callback: (data: string) => void): { dispose: () => void };
  onResize(
    callback: (dims: TerminalDimensions) => void,
  ): { dispose: () => void };
  fit(): TerminalDimensions;
  focus(): void;
  dispose(): void;
}
```

- **`GhosttyWebAdapter` (標準実装)**:
  - `@coder/ghostty-web` をラップ。
  - WASMによる高速なANSI/VT100シーケンス解釈とCanvas
    2D描画により、ビルドログ等の大量出力でもUIスレッドをブロックしません。
- **差し替え容易性 (Pluggable)**:
  - 将来的にAndroid
    PWAでのネイティブ文字選択・IME入力の操作性やアクセシビリティを重視したいケースが生じた場合でも、DOMレンダラを採用する
    `wterm` (`@wterm/ghostty`) 等のアダプタへ最小限の修正で切り替え可能です。
- **React コンポーネント (`TerminalView`)**:
  - `ResizeObserver` による自動 `fit()` 実行とノード側PTYへの `resize`
    メッセージ送信。
  - モバイル仮想キーバー（タップで `Ctrl`, `Esc`, `Tab`,
    矢印キー等のエスケープコードを送信）の統合。

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
- [ ] セッション画面の実装:
  - [ ] チャットタイムライン（思考折りたたみ・スラッシュコマンド補完・承認Inbox）
  - [ ] 2階層 Diff ビューア（セッション編集差分 + Worktree リアルタイムGit差分:
        `vs Base` / `vs HEAD`）
  - [ ] 双方向 Web ターミナル（`ITerminalAdapter` 抽象化 + `ghostty-web` 実装 +
        ConPTY/Unix PTY WebSocket 直結 + モバイル仮想キーバー）
- [ ] プロジェクト & Worktree 管理画面（Worktree 一覧・新規作成・削除）
- [ ] Service Worker (`sw.js`) と `web-push` クレートによる VAPID
      Push通知（Androidバックグラウンド通知＆バナー承認）の実装
- [ ] `fxg service install` による各OS自動起動設定の実装
