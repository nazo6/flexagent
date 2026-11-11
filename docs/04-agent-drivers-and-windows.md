# 04. エージェントドライバ実装 (ACP一本化) と Windows 対応詳細

`crates/fxg-acp` および `crates/fxg-pty`
におけるエージェント制御・プロセス管理の具体的な実装設計です。

---

## 1. `AgentDriver` トレイトによる抽象化

すべてのアクション・対話は `AgentDriver` 非同期トレイトで統一します。

```rust
#[async_trait::async_trait]
pub trait AgentDriver: Send + Sync {
    /// ドライバ識別子 ("acp")
    fn driver_kind(&self) -> &'static str;

    /// セッションを起動し、イベント送出用のストリーム/ハンドルを返す。
    /// `req.resume` 指定時はネイティブ復元 (`session/resume` / `session/load`)
    /// を試み、成否を `StartedSession::context_restored` で返す (§4.1)
    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::Sender<DriverEvent>,
    ) -> anyhow::Result<StartedSession>;
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

### 1.1 エージェント実装の3層モデル（Phase 1 実装済み）

ACP だけでは、アイコンや起動オプションなどのエージェント固有メタデータや、
ACP に存在しない機能（独自 API によるモデル名取得など）を扱えない。将来の
拡張を `agent_id` 分岐なしで追加できるよう、実装を次の3層に分ける。
Phase 1（`AgentCatalog`）と Phase 2（Tier 2 の基盤）は実装済みで、Tier 3 と
Tier 2 の実エージェント（opencode）は計画中である。

| 層                   | 実行時の実装                           | ACP の再利用 | エージェントごとに追加するコード            |
| :------------------- | :------------------------------------- | :----------- | :------------------------------------------ |
| Tier 1: ACP only     | `AcpDriver`                            | すべて       | なし（registry / preset / custom 定義のみ） |
| Tier 2: ACP + custom | `ExtendedAcpDriver` + `AgentExtension` | すべて       | `AgentExtension` の差分のみ                 |
| Tier 3: Full custom  | `impl AgentDriver`                     | なし         | 起動・イベント変換を含む driver 実装        |

- エージェント固有の知識は `AgentCatalog` と extension に閉じ込め、
  `SessionManager` と `AcpDriver` には追加しない。
- `AgentCatalog` が **custom 定義 → builtin provider → ACP Registry** の順で
  起動定義と backend を解決する。
- `AgentLaunchSpec` は program / args / env だけの純粋データとし、warm pool
  のキーとして扱う。extension の状態は含めない。
- Tier 2 は `ExtendedAcpDriver` が ACP イベントを中継し、
  `CapabilitiesUpdated` に `CapabilityOverlay` を適用してから
  `SessionExtension` に通す。capability probe も同じ経路を通る。
- ユーザー定義のカスタム ACP エージェント（§3）は Tier 1 のまま。
  将来 `CustomAgentConfig.extension` で builtin extension を参照できるようにする。
- Tier 3 の builtin ID は予約し、custom 定義による上書きを拒否する。

型設計・実装フェーズ・変更対象の詳細は
[`docs/changelog/2026-11-11-agent-extension-architecture.md`](./changelog/2026-11-11-agent-extension-architecture.md)
を参照する。

---

## 2. 任意ACPエージェント対応 (`AcpDriver` & Registry Manager)

### 2.1 ACP Registry カタログとローカル起動解決

1. **レジストリ取得 (カタログ用途)**:
   - 公式インデックス
     `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`
     を取得し、`~/.flexagent/cache/registry.json` にキャッシュ（TTL
     24時間、Web UI のカタログ更新で即時更新）。
   - レジストリは「表示名・説明・配布形態を知るためのカタログ」として使い、
     起動できるのは **このノードに実体があるものだけ** とする (未登録 ID を
     `PATH` から暗黙解決するフォールバックは行わない。任意のコマンドは
     §3 のカスタムエージェントとして明示登録する)。
2. **起動解決の優先順位 (ローカル優先)**:
   1. カスタム定義 (`node.db` の `custom_agents`)
   2. **ビルトイン preset**: 既知の CLI が `PATH` にあれば、その ACP
      起動引数で起動する (`opencode acp` / `agy acp`)。ユーザーが同名の
      custom を定義した場合は custom が優先される。
   3. `PATH` 上の既存インストール (`which::which`、Windows は `PATHEXT`
      対応)
   4. `npx` 配布 (`npx -y <package> <args...>`)
   5. `uvx` 配布 (`uvx <package> <args...>`)
   - 代表例: `fxg session new antigravity` は `agy acp`、`fxg session new opencode` は
     `opencode acp` として起動する。registry の `antigravity-acp` が
     `agy_acp_server.par` を前提としていても、`agy` CLI が PATH にあれば
     preset が優先される。
3. **インストールは明示操作のみ**:
   - fxg はバイナリ配布のダウンロード・展開を行わない。binary 配布のみの
     エージェントは、各ツールの配布元から導入して `PATH` に置くか、custom
     定義で起動コマンドを指定する。
   - 未導入のまま起動しようとした場合は、インストール方法を含むエラーを返す
     (`PATH` に既存インストールがあればそれを使うため、通常は導入不要)。
   - `npx` / `uvx` は実行時にパッケージマネージャが取得するため、fxg 側では
     成果物を保持しない (Web UI の無効化の対象外)。
   - 実体を持たない `npx` / `uvx` 配布は **既定では一覧に出さない**。Web UI の
     「カタログから追加」でエージェントごとに**有効化**し、状態は
     `node.db` の `enabled_package_agents` に保存する
     (`AgentAction::SetPackageEnabled`)。有効化は一覧・選択肢の制御のみで、
     起動解決 (`launch_spec`) は引き続き `npx` / `uvx` を利用できる。

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
  - `UsageUpdate` ➔ コンテキスト使用量 (`used` / `size`) と累積コスト
    (`cost`) を `UnifiedEventPayload::UsageUpdated` として記録する (ハブ側
    `sessions.usage_json` 投影。累積ではなく最新値を保持)。
  - ターン終了応答の `StopReason` ➔ `EndTurn` / `Cancelled` は正常終了として
    イベント化しない。`MaxTokens` / `MaxTurnRequests` / `Refusal` は
    `UnifiedEventPayload::TurnEnded` として `reason` コードのみを記録し、
    UI / CLI がコードから文言を導出してシステム行で表示する。
- **`request_permission(req) -> oneshot::Receiver<RequestPermissionResponse>`**:
  - `oneshot::channel`
    を生成してMapに保持し、`UnifiedEventPayload::PermissionRequest` を発行。
  - Web Push通知（Android PWA）およびWebSocketへ即時ブロードキャスト。
  - ユーザーがAndroid・Web・CLIのいずれかで承認を選択したら、`oneshot::Sender`
    に結果を流してACPエージェントの処理を再開。
- **`elicitation_create(req) -> oneshot::Receiver<CreateElicitationResponse>`**
  (stable v1):
  - エージェントの「質問」ツールを `UnifiedEventPayload::ElicitationRequest`
    として記録し、Web / Android / CLI へブロードキャストする。回答は
    `accept` (form content) / `decline` / `cancel` の 3 択。
  - Phase 1 は form モードのみ広告する
    (`clientCapabilities.elicitation.form`)。`accept` の content は送信前に
    クライアントが `requested_schema` に対して軽量検証し、ターンキャンセル時は
    未解決の elicitation を `cancel` で解決する (仕様 MUST)。
- **認証 (`authMethods` / `authenticate` / `logout`)**:
  - `initialize` の `authMethods` を `AgentAuthMethodInfo` として保持し、
    `session/new` が `-32000` (`auth_required`) を返した場合は
    `AUTH_REQUIRED` + methods をクライアントへ返す。
  - `clientCapabilities.auth.terminal = true` を広告する。terminal 型は
    `fxg agents login <agent>` で「起動コマンド + method.args」を対話実行する
    (`agy acp --login` など)。agent 型は `authenticate` を送る。
  - `auth.logout` を広告しているエージェントは `fxg agents logout` で
    `logout` を送る。
- **`read_text_file` / `write_text_file`**:
  - セッションの `local_path`
    基準でファイルを読み書き。書き込み時は変更前後の内容からUnified
    Diffを計算してイベントに添付。
- **`terminal_create` / `terminal_output` / `terminal_wait_for_exit` /
  `terminal_kill`**:
  - `fxg-pty` クレートを呼び出し、ConPTY (Windows) または Unix PTY
    でコマンドを実行し、出力をリアルタイムにストリーム配信。

---

## 3. カスタム ACP エージェント (任意コマンド)

ACP Registry に載っていないエージェントや自前の ACP 実装は、**明示的なカスタム
定義**として登録して起動する。

1. **定義方法**:
   - Web UI (設定 > エージェント管理 > カスタム追加) または API から
     `node.db` の `custom_agents` テーブルへ保存する (`name` / `command` /
     `args` / `env` / `env_secrets`)。
   - API: `PUT /api/v1/nodes/:node_id/agents/custom/:agent_id` (upsert) /
     `DELETE /api/v1/nodes/:node_id/agents/custom/:agent_id`。
   - 実行時設定はノードの DB にのみ保存し、ハブへ同期しない。
2. **解決と起動**:
   - カスタム定義は常に Tier 1 (`AcpDriver`) として解決する（§1.1）。
   - `command` は `PATH` 上のコマンド名または絶対パス。シェル経由ではないため
     グロブ・`~`・環境変数の展開は行わない。
   - 起動直前に `fxg_pty::resolve_command` が `which` (`PATHEXT`) で解決し、
     `args` / `env` を付けて ACP プロセスとしてスポーンする。
3. **Tier 2 extension の再利用（計画）**:
   - `CustomAgentConfig.extension` で builtin extension を参照できるようにする
     （§1.1）。指定時は `DriverKind::AcpExtended` となり、custom の起動定義に
     extension の `prepare_launch` を適用する。
   - 未知の extension ID は保存時と起動時に拒否する。
4. **一覧表示と優先順位**:
   - カスタム定義は常に「利用可能」として扱い、`sources: ["custom"]` で返す。
   - 同じ ID の builtin / registry より優先し、重複表示しない。
   - Tier 3 の builtin ID は予約し、custom 定義による上書きを拒否する。

---

## 4. セッションの Fork（ACPネイティブのみ）

**Fork** は、ACP の `session/fork` + `session/load` をそのまま使う場合にのみ提供します。
fxg 側で履歴を組み立て直す Replay フォールバックは行いません。

- 対象は **同じエージェント・同じノード上にあるセッション** のみです。現在のセッション全体を
  分岐し、新しい `session_id` を作成します。任意のメッセージ地点を指定した Fork は
  ACP に存在しないため提供しません。
- エージェントが `sessionCapabilities.fork` と `loadSession` の両方を広告した場合だけ
  実行できます。未対応の場合は `FORK_UNSUPPORTED` を返し、Web UI では Fork ボタンを
  表示しません（`CapabilitiesUpdated.fork_supported` を `sessions.fork_supported` に投影）。
- 新しいセッションは `parent_session_id` だけを系譜として記録します。会話はエージェントが
  保持しているため、Replay 用の `UserMessage` は追加しません。
- 別ノード・一時VM・別エージェントへの Fork（Git bundle / 履歴引き継ぎ）は行いません。

### 4.1 セッションのレジューム (Resume)

停止したセッション (`fxg node restart`・`fxg session kill`・エージェントプロセス
異常終了) を、**同一 `session_id` のまま**会話コンテキストを維持して再開します
(CLI `fxg session resume` / REST `POST /api/v1/sessions/:id/resume` / Web UI)。

1. **再開可否の判定**: `SessionManager` の active 一覧 (実際の稼働状況) を正と
   します。強制終了時は `stopped` への遷移イベントが書かれないため、DB の
   `status` が `idle` / `running` のままでも再開を許可します。一方、
   `provisioning` (一時VM) は v1 では拒否します
   (`INVALID_STATE`)。
2. **ネイティブ復元 (優先)**: エージェント内部の会話コンテキストを維持したまま
   再開します。
   - `sessionCapabilities.resume` → `session/resume` (履歴 Replay なし)、
     非対応で `agentCapabilities.loadSession` → `session/load` を使用します。
     `session/load` の Replay 通知は fxg のイベントログと重複するため破棄します
     (Replay は load 応答前に完了する仕様を利用して応答直後に drain)。
3. **Replay フォールバック**: ネイティブ復元できない場合は、
   履歴 (会話 + ツール要約) を `client_source = "resume"` の `UserMessage`
   として新エージェントセッションへ注入します。
4. **イベント**: `SessionCreated` は再発行せず、`StatusChanged(Idle)` を追記して
   状態投影を復帰させます。ネイティブ復元できたかは API 応答
   (`context_restored`) でクライアントへ返します。

#### 4.1.1 送信時の自動レジューム (ネイティブ限定)

停止済みセッションへプロンプトを送信した場合、ユーザーが明示的に Resume
しなくても、ノードが**ネイティブ復元のみ**で自動再開してからプロンプトを
送信します (`SessionManager::send_prompt` → `auto_resume_for_prompt`)。

- **ネイティブ限定**: `ResumeRequest { allow_fresh: false }` で起動し、復元
  できない場合は新規セッションを作成せず、ドライバが
  `NativeResumeUnavailable` を返します。ノードはこれを
  `ErrorCode::RESUME_REQUIRED` としてクライアントへ伝えます
  (セッションは `stopped` のまま。履歴 Replay は暗黙実行しません)。
- **履歴 Replay は明示操作**: `RESUME_REQUIRED` を受けたクライアント
  (Web UI のコンポーザ) は「履歴を引き継いで再開して送信」を提案し、
  `SessionResume` (Replay フォールバック) を実行してから再送します。
- **排他**: 明示 `resume` と同じ `resuming` ガードで直列化し、他の再開処理が
  進行中の場合は完了を待ってから送信します。
- **タイムアウト**: 自動レジュームはエージェントのコールドスタート (~25 秒) を
  含みうるため、送信コマンドの中継タイムアウトを 120 秒としています
  (`fxg-hub` の `SESSION_COMMAND_TIMEOUT`。Client WS 経由では 1 コマンド
  1 応答の同期処理のため、そのクライアント接続は完了まで待ちます)。

> **v2 (将来拡張)**: 一時VM (provisioner) セッションの再開 (VM 再起動 +
> ワークスペース引き継ぎ) と、`fxg node` 起動時の自動レジューム。

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
        info.limit_kill_on_job_close(); // ノード終了時・Drop時に孫プロセスまで確実にKill
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

- **コマンド解決**: `npx`, `uvx`, `claude` などを起動する際、必ず
  `which::which_in(cmd, env::var_os("PATH"), &cwd)`
  を通すことで、`claude.cmd` や `npx.cmd` の拡張子を確実に解決してから
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

`fxg service install [node|hub]` コマンドにより、各OS標準のユーザー権限バックグラウンドサービスとして
ノードまたはハブを登録します（既定は `node`）。`service` は **OS への登録・解除**を担当し
（`install` は登録後に起動する）、起動・停止・状態確認は `fxg node start|stop|status` /
`fxg hub start|stop|status` が行います。unit が登録済みの場合、これらのコマンドは
systemd / launchd / Task Scheduler に委譲します。

- **Windows**: タスクスケジューラ（ログオン時実行・コンソールウィンドウ非表示
  `CREATE_NO_WINDOW`）またはスタートアップ登録。
  - 実装: `schtasks /Create /SC ONLOGON` を第一手段とする。管理者権限が無い
    環境では `Access is denied` となるため、スタートアップフォルダ
    (`%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\fxg-node.vbs`)
    へ VBS ランチャー (`WScript.Shell.Run ..., 0, False` = ウィンドウ非表示)
    を書き出すフォールバックを持つ。状態確認は `fxg node status` が行う。
- **Linux / WSL**: `~/.config/systemd/user/fxg-node.service` を生成し
  `systemctl --user enable --now fxg-node`。
- **macOS**: `~/Library/LaunchAgents/dev.flexagent.node.plist` を生成し
  `launchctl load`。

ハブを常駐させる場合は `fxg service install hub` を使います（unit 名は
`fxg-hub` / `dev.flexagent.hub`）。`fxg service uninstall [node|hub]` で停止・解除します。

### 5.5 タスクトレイ常駐 (`fxg node start --tray`)

`fxg node start --tray` で起動すると、ノードはタスクトレイに常駐し、メニューから
状態確認と操作ができます（既定は無効。`~/.flexagent/config.toml` の
`[node] tray = true` で常時有効化、`--no-tray` で一時的に無効化）。
`fxg internal stdio`（一時VM・プロビジョナー）では常に無効です。

- **メニュー**:
  - ステータス表示: ハブ接続状態（●接続中 / ○スタンドアロン）、
    未同期 Outbox 件数、稼働中セッション数（3秒間隔で更新）
  - 「ログイン時に自動起動」: `fxg service install` / `uninstall` と同一実装の
    チェック項目（登録後は実状態を再確認して表示を同期）
  - 「Web UI を開く」: `fxg web` と同じトークン付き URL を既定ブラウザで開く
  - 「終了」: `NodeDaemon` の graceful shutdown（エージェント/PTY 停止 →
    `nodes.is_online = 0` 記録）を実行して終了
- **アイコン**: `ui/static/icon-192.png` を 32×32 へ縮小し、右下に接続状態ドット
  （緑=ハブ接続中 / グレー=スタンドアロン）を合成する。
- **状態の取得元**: プロセス内の `DaemonState`（`node.db` / 同期ワーカー）を直接
  参照する（HTTP を経由しない）。
- **OS 別実装**（`tray-icon` はイベントループを自前で持たないため）:
  - **Windows** (`crates/fxg-cli/src/tray/windows.rs`): 専用スレッドで Win32
    メッセージループ（`GetMessageW`）を回し、状態更新は `PostThreadMessageW`
    で起床する。
  - **Linux** (`tray/linux.rs`): 純 Rust の D-Bus
    バックエンド（`ksni`）を使うため
    イベントループ不要。専用スレッド + チャネル駆動でメニューを操作する
    （GTK 依存なし。GNOME では AppIndicator 拡張が必要。D-Bus セッションが
    無い環境ではトレイなしで稼働）。
  - **macOS** (`tray/macos.rs`): `NSStatusItem`
    はメインスレッドのイベントループ上で
    作成・操作する必要があるため、`main.rs` の早期分岐でメインスレッドを tao
    ループに明け渡し、ノード本体（tokio
    ランタイム）をワーカースレッドで起動する。
- **スレッド安全性**: `muda` のメニュー項目は `!Send` のため、メニューの操作は
  すべてトレイを作成したスレッド上で行う（ノードとはチャネルで連携）。
- **フェイルセーフ**: トレイを作成できない環境では警告のみでノードは稼働を
  継続する（起動の成否はノード本体と独立）。

### 4.4 承認ポリシー (`PermissionPolicy`)

セッション単位で `ask` / `auto_allow_all` を切り替えられます。
`auto_allow_all` では、エージェントが `request_permission` で提示した
`allow_*` オプション (`allow_always` を優先) をノードが自動選択し、
`PermissionResolved` を `resolved_by = "auto"` として記録します。
ポリシーは `SessionPermissionPolicyChanged` イベントで
`sessions.permission_policy` に投影され、Web UI の Composer の
`Auto` トグルから変更できます。

### 4.5 プロジェクト別の初期設定

Web UI はプロジェクトごとに最後に選択した `configOptions`
(モデル / thinking level 等) を localStorage に保存し、新規セッション作成時に
`CreateSessionRequest.initial_config` として送ります。ノードは
`session/new` 後に `session/set_config_option` を順に送って適用します。

### 4.6 新規セッション前の能力プローブ

ACP は `initialize` ではモデル一覧を返さず、`session/new` のレスポンスに含まれる
`configOptions` が唯一の情報源です。そこで Web UI は
`POST /api/v1/nodes/:node_id/agents/:agent_id/probe` を呼び、ノードが
**一時的な ACP セッション**を1回だけ開始して能力を取得します。

- **DB 非汚染**: プローブは `SessionManager::probe_agent_capabilities` が
  driver を直接使い、`sessions` 行やイベントを記録しない。
- **ウォームプール再利用**: 既存の `AcpDriver` / warm pool を使うため、プローブで
  起動したプロセスを実セッションが再利用でき、初回起動コストを相殺できる。
- **キャッシュ**: `(agent_id, local_path)` 単位で約10分保持する。実セッションの
  `CapabilitiesUpdated` でもキャッシュを更新する。同時プローブは直列化する。
  UI の「Refresh」は `force: true` でキャッシュを迂回する。
- **失敗時**: `AUTH_REQUIRED` は通常のログイン復旧導線へ流す。タイムアウトや
  起動失敗時も新規セッション自体は開始でき、モデルはエージェント既定値を使う。
- **制約**: 一時VMはノード起動前なのでプローブできない。また、エージェント側に
  空のセッションが残る実装がある (ACP にセッションを作らない一覧 API はない)。
- **新規セッション UI**: モデル pill は常に表示し、取得中は `Default` のまま開始を
  ブロックしない。`category = "mode"` の `configOption` は既存の
  `CreateSessionRequest.mode` と重複するため、モデル pill 列には出さない。
