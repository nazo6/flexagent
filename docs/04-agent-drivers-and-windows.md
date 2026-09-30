# 04. エージェントドライバ実装 (`ACP` / `opencode2`) と Windows 対応詳細

`crates/fxg-acp` および `crates/fxg-pty`
におけるエージェント制御・プロセス管理の具体的な実装設計です。

---

## 1. `AgentDriver` トレイトによる抽象化

将来的にACP以外の独自プロトコルを持つエージェントが増えても `fxg-node`
本体を変更せずに済むよう、以下の非同期トレイトで統一します。

```rust
#[async_trait::async_trait]
pub trait AgentDriver: Send + Sync {
    /// ドライバ識別子 ("acp", "opencode2")
    fn driver_kind(&self) -> &'static str;

    /// 新規セッションを起動し、イベント送出用のストリーム/ハンドルを返す
    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::Sender<DriverEvent>,
    ) -> anyhow::Result<Box<dyn ActiveSessionHandle>>;
}

#[async_trait::async_trait]
pub trait ActiveSessionHandle: Send + Sync {
    /// プロンプト（通常メッセージまたはスラッシュコマンド）の送信
    async fn send_prompt(&self, text: String) -> anyhow::Result<()>;

    /// 権限リクエストへの応答
    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()>;

    /// モード変更 (`plan`, `code` 等)
    async fn set_mode(&self, mode_id: String) -> anyhow::Result<()>;

    /// 設定変更 (モデル選択等)
    async fn set_config(&self, key: String, value: serde_json::Value) -> anyhow::Result<()>;

    /// 現在のターンの中断 (Interrupt / Cancel)
    async fn cancel_turn(&self) -> anyhow::Result<()>;

    /// セッションプロセスの完全終了
    async fn shutdown(&self) -> anyhow::Result<()>;
}
```

---

## 2. 任意ACPエージェント対応 (`AcpDriver` & Registry Manager)

### 2.1 ACP Registry からの自動インストール・キャッシュ

1. **レジストリ取得**:
   - 公式インデックス
     `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`
     を取得し、`~/.flexagent/cache/registry.json` にキャッシュ（TTL
     24時間、`fxg agents update` で即時更新）。
   - ユーザー定義のカスタム `agent.json`（ローカルパスまたはURL、例:
     `antigravity-acp/agent.json`）も `~/.flexagent/config.toml` に追加可能。
2. **配布形態 (`distribution`) ごとの起動解決**:
   - **`binary`**: 現在のOS/Arch（`windows-x86_64`, `darwin-aarch64`,
     `linux-x86_64`
     等）に対応するアーカイブURLをダウンロードし、`~/.flexagent/agents/<agent-id>/<version>/`
     に展開して実行バイナリパスを解決。
   - **`npx`**: `which::which("npx")`（Windowsでは `npx.cmd`
     を自動解決）を用いて `npx -y <package> <args...>` を起動。
   - **`uvx`**: `which::which("uvx")` を用いて `uvx <package> <args...>`
     を起動。

### 2.2 `agent-client-protocol` クレートを用いた `Client` 実装

ACPでは、エディタやオーケストレータ側が **`acp::Client` トレイト**
を実装してサブプロセスの `stdin/stdout` に接続します。

`fxg-acp` の `FxgAcpClient` が処理する主要コールバック：

- **`session_update(notification)`**:
  - `AgentMessageChunk`, `AgentThoughtChunk` ➔
    リアルタイム配信チャネルへ即時送出しつつ、バッファに蓄積。
  - `ToolCall`, `ToolCallUpdate` ➔ `locations` や `content` (Diff) を抽出し
    `UnifiedEventPayload::ToolCall` として `node.db` へ記録。
  - `AvailableCommandsUpdate` ➔
    スラッシュコマンド一覧を更新し、UIとCLIの補完リストに反映。
  - `CurrentModeUpdate` / `ConfigOptionUpdate` ➔
    現在のモードやモデル設定選択肢をUIへ同期。
- **`request_permission(req) -> oneshot::Receiver<RequestPermissionResponse>`**:
  - `oneshot::channel`
    を生成してMapに保持し、`UnifiedEventPayload::PermissionRequest` を発行。
  - Web Push通知（Android PWA）およびWebSocket/IPCへ即時ブロードキャスト。
  - ユーザーがAndroid・Web・CLIのいずれかで承認を選択したら、`oneshot::Sender`
    に結果を流してACPエージェントの処理を再開。
- **`read_text_file` / `write_text_file`**:
  - セッションの `local_path`
    基準でファイルを読み書き。書き込み時は変更前後の内容からUnified
    Diffを計算してイベントに添付。
- **`terminal_create` / `terminal_output` / `terminal_wait_for_exit` /
  `terminal_kill`**:
  - `fxg-pty` クレートを呼び出し、ConPTY (Windows) または Unix PTY
    でコマンドを実行し、出力をリアルタイムにストリーム配信。

---

## 3. OpenCode / OpenCode2 ハイブリッド統合 (`OpenCode2Driver`)

`fxg run opencode`（または
`fxg run opencode2`）を実行した際、最も快適かつ高機能に使えるよう2つのモードを提供します。

### モードA: Server Bridge + 純正TUI Attach モード（デフォルト推奨）

OpenCode / OpenCode2
のクライアント・サーバー分離アーキテクチャをフル活用します：

1. `fxg daemon` がバックグラウンドで
   `opencode2 serve --hostname 127.0.0.1 --port <free_port>`
   を起動（セキュリティのためランダムな `OPENCODE_SERVER_PASSWORD`
   を自動生成して環境変数に注入）。
2. `fxg daemon` は HTTP (OpenAPI) + SSE (`/event` ストリーム)
   クライアントとしてローカルの `opencode2 serve`
   に接続し、すべてのメッセージ・ツール実行・権限要求を `UnifiedEventPayload`
   に変換して `node.db` および中央サーバーへ同期します。
3. **ユーザーがPCターミナルで `fxg run opencode` を叩いた場合**: `fxg` CLI
   はローカルの `opencode2 serve` に対して
   `opencode2 run --server http://127.0.0.1:<port> --session <id>`
   を実行します。
   - **結果**: PCのターミナルでは **100%純正のOpenCode2 TUI**
     がそのまま動き、同時にスマホ（Android
     PWA）やWebブラウザからも同じセッションがリアルタイムに見えて双方向操作できます。

#### サーバーライフサイクルと設計判断 (2026-10-01 確定)

opencode2 本体は「バックグラウンドサービス」1つを全セッション・全 TUI
で共有する設計だが、**fxg は分離を優先し、fxg セッションごとに専用の
`opencode2 serve`（空きポート +
ランダムパスワード）を起動する**（現状維持と決定）。
これにより:

- あるセッションの `kill`（プロセスツリー終了）が他の fxg
  セッションやユーザー自身の opencode2 に波及しない。
- 純正TUI を閉じただけではサーバーは終了しない（デーモンが保持し、
  `fxg attach` で再接続可能）。
- `fxg daemon` の終了（正常終了は `shutdown_all`、Windows
  の強制終了は Job Object の kill-on-close）で全セッションが停止する。
  実行中ターン・承認待ちは失われるが、**会話履歴は opencode2 のグローバル DB
  (`~/.local/share/opencode/opencode.db`) に永続化されるため失われない**
  （新サーバーから過去セッションを取得できることを実証済み）。
- 全サーバーが同一のグローバル DB を共有するため、複数サーバー同時稼働時は
  SQLite の書き込みロック競合が起こり得る（WAL のため kill による破損はない）。

将来「ノード共有サーバー」（デーモンが1サーバーを管理）へ変更する場合は、
`fxg session kill` を「エンジンセッションの abort + active
一覧からの除去」に変え、サーバー自体はデーモン終了時まで維持する必要がある。

### モードB: ACP モード (`opencode2 acp`)

`opencode2 acp`
サブコマンドを使って標準ACPエージェントとして起動するモードです。Web/Androidからヘッドレスで起動する場合や、`AcpDriver`
と完全に同じ挙動に揃えたい場合に使用します。

---

## 4. セッションの Fork（分岐）と Revert（巻き戻し）の実装設計

特定のメッセージ時点から別ルートを試す **Fork**
と、エージェントが行ったファイル変更ごと過去のターンへ巻き戻す **Revert (Undo)**
を、全エージェント共通でサポートします。

### 4.1 ターンごとの軽量ファイルスナップショット (Shadow Git Tree 方式)

ユーザーの実際の `.git`（ブランチや
`git log`、ステージング状態）を一切汚さずに、任意のターン時点のファイル状態をミリ秒単位で保存・復元するため、Gitのプラミングコマンドと専用Indexファイル（`GIT_INDEX_FILE`）を用いた
**Shadow Git Tree** を実装します。

1. **スナップショットの取得（各ターンの `send_prompt` 直前に自動実行）**:
   - 環境変数 `GIT_INDEX_FILE=~/.flexagent/snapshots/<session-id>.index`
     を指定した状態で：
     1. `git add -A`（未追跡ファイルも含めてシャドウIndexにステージング）
     2. `git write-tree` を実行し、返ってきた **40文字の Tree Hash
        (`snapshot_tree_hash`)** をその `UserMessage` イベントの
        payload（`UnifiedEventPayload::UserMessage.snapshot_tree_hash`）に含めて
        イベントログ（`node.db` / `server.db` の
        `session_events.payload_json`）へ保存します。 イベント payload
        が唯一の正であり、専用カラムへの複製は行いません。
   - **シャドウIndexはセッション単位で分離**します（`<session-id>.index`）。同一リポジトリの複数セッション（並行
     Worktree 作業）が同時にスナップショットを取得しても Index
     ファイルを奪い合わないためです。同一セッション内のターンは直列処理されるため競合しません。
   - シャドウGit操作（`git add -A` / `read-tree` / `checkout-index`
     等）は、常に `-c core.autocrlf=false -c core.eol=lf`
     を前置して実行します。ワークツリーのバイト列（LF / CRLF）を変換せずに
     スナップショット・復元するためです（Windows の既定
     `core.autocrlf=true` では、復元時に LF が CRLF へ書き換えられ
     ワークスペースが破壊される）。
   - `git add -A` は `.gitignore`
     対象を除外しますが、巨大な未追跡ファイル（ビルド成果物等）が含まれ得るため、サイズ上限（例:
     100 MB 超）を設けて超過時はスナップショットをスキップし警告ログを残します。
   - コミットオブジェクトすら作らないためユーザーのブランチ履歴は一切汚れず、変更がないファイルはGitオブジェクトDB内で自動的に重複排除されます。
2. **Revert（指定したメッセージ時点へのファイル復元 ＋ 会話巻き戻し）**:
   - ユーザーがWeb / Android /
     CLIで特定のメッセージを選び「ここまでRevert（巻き戻し）」を実行した場合：
     1. 対象メッセージの `snapshot_tree_hash`
        を使い、ワークスペースのファイルをその時点へ復元（直前の状態の退避バックアップTreeも自動作成してデータロストを防止）。
     2. エージェント側の会話をそのメッセージ時点へ巻き戻します（OpenCode2ならネイティブの
        `POST /session/{id}/revert`
        API、一般ACPエージェントならその時点までの履歴でセッションを再構築）。

### 4.2 セッションの Fork（会話の分岐）

任意のメッセージ（`node_seq`）時点から会話を分岐させ、新しい `session_id`
を作成します：

1. **OpenCode2 の場合**: OpenCode2のネイティブAPI
   `POST /session/{id}/fork`（指定 `messageID`
   からの分岐）を呼び出し、内部コンテキストを維持したまま子セッションを生成します。
2. **任意ACPエージェントの場合（および別エージェントへの乗り換えFork）**:
   - エージェントがACPの `unstable_session_fork`
     に対応していればそれを呼び出します。
   - 未対応のエージェント、または **「途中まで `opencode2`
     で進めた会話を、ここから `antigravity-acp` に切り替えてForkする」**
     といった場合は、`node.db` / `server.db` に保存されている `node_seq`
     までの構造化イベント履歴（会話＋変更ファイル要約）を新しいACPセッションの初期コンテキストとして自動注入（Replay）します。

---

## 5. Windows サポートと PTY 双方向制御 (`crates/fxg-pty`)

### 5.1 Windows Job Object によるプロセスツリー完全終了

Windowsでは `node.exe` や `.cmd` ラッパー経由で起動したエージェントを
`Child::kill()`
しても、孫プロセス（言語サーバーやテストランナー、開発サーバー）が生き残りファイルをロックする問題が多発します。
これを防ぐため、すべてのエージェントプロセスとターミナルプロセスを **Windows Job
Object** にバインドします：

```rust
#[cfg(windows)]
pub struct WinJobGuard {
    job: win32job::Job,
}

#[cfg(windows)]
impl WinJobGuard {
    pub fn new_kill_on_close() -> anyhow::Result<Self> {
        let job = win32job::Job::create()?;
        let mut info = job.query_extended_limit_info()?;
        info.limit_kill_on_job_close(); // デーモン終了時・Drop時に孫プロセスまで確実にKill
        job.set_extended_limit_info(&info)?;
        Ok(Self { job })
    }

    pub fn assign_process(&self, process_handle: std::os::windows::io::RawHandle) -> anyhow::Result<()> {
        self.job.assign_process(process_handle as isize)?;
        Ok(())
    }
}
```

### 5.2 コマンド解決 (`which` + `PATHEXT`) とパス正規化 (`dunce`)

- **コマンド解決**: `npx`, `uvx`, `opencode2` などを起動する際、必ず
  `which::which_in(cmd, env::var_os("PATH"), &cwd)`
  を通すことで、`opencode2.cmd` や `npx.cmd` の拡張子を確実に解決してから
  `tokio::process::Command` に渡します。
- **UNCパス回避**: Windowsで `std::fs::canonicalize` を使うと `\\?\D:\ghq\...`
  というUNCプレフィックスが付き、Node.js製エージェントや外部ツールがパス解釈に失敗することがあります。そのため、パス正規化には必ず
  **`dunce::canonicalize`** を使用し、通常の `D:\ghq\...` 形式を維持します。

### 5.3 双方向 Web PTY セッションマネージャ (`PtySessionManager`)

GUI (Web UI / スマホPWA)
からの対話シェル操作、および対話コマンド実行を実現するため、`portable-pty`
をラップした双方向 PTY 管理層を提供します。

- **クロスプラットフォーム PTY**: Windows では ConPTY (`portable-pty::conpty`),
  macOS/Linux では Unix PTY (`portable-pty::unix`) を自動選択。
- **ConPTY 起動ハンドシェイク** (Windows): ConPTY は
  `PSEUDOCONSOLE_INHERIT_CURSOR` により起動直後にカーソル位置照会 (`ESC[6n`)
  を送り、応答 (`ESC[1;1R`) が届くまで**子プロセスのコンソール操作をブロック**
  します。ヘッドレス実行 (CI / エージェント駆動) でも停止しないよう、
  マネージャ (`ConPtyStartupHandshake`) が照会へ応答し、照会シーケンスは
  出力から除去します (アプリ自身が発行する 2 回目以降の照会は、フロントエンド
  が応答できるよう透過させます)。
- **非同期ストリーミング**: PTY の Master `Read` を非同期ループで読み取り
  WebSocket (`PtyOutput`) へブロードキャストし、クライアントからのキー入力
  (`PtyInput`) を Master `Write` に即時フラッシュ。終了検知は子プロセスの
  `wait()` 専用スレッドが担い、`Exit` は残存出力の配信完了後に配信します
  (Windows の ConPTY は子プロセス終了だけでは EOF にならないため)。
- **動的リサイズ**: 端末の画面サイズ変更に合わせて
  `master.resize(PtySize { rows, cols, .. })` を即時適用。
- **対話型ACPコマンドへの直接入力**:
  エージェントが実行したプロセスが標準入力を待機している場合（ACP
  `terminal_create`
  で作成されたPTY）、UI上のターミナルからキーストローク（`TerminalInput`）を注入して対話操作を完結。

### 5.4 バックグラウンド常駐化 (`fxg service`)

`fxg service install`
コマンドにより、各OS標準のユーザー権限バックグラウンドサービスとしてデーモンを登録します：

- **Windows**: タスクスケジューラ（ログオン時実行・コンソールウィンドウ非表示
  `CREATE_NO_WINDOW`）またはスタートアップ登録。
- **Linux / WSL**: `~/.config/systemd/user/fxg-daemon.service` を生成し
  `systemctl --user enable --now fxg-daemon`。
- **macOS**: `~/Library/LaunchAgents/dev.flexagent.daemon.plist` を生成し
  `launchctl load`。
