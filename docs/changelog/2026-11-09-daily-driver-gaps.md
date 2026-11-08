# 2026-11-09 日常開発利用に向けたギャップ分析と実装計画

- **日付**: 2026-11-09
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `fxg-node`, `fxg-server`,
  `fxg-cli`, `ui`
- **対象スクリプト / 設定**: `mise run check` / `mise run fmt` /
  `mise run types` / `mise run sqlx:prepare` /
  `cargo test -p fxg-acp --lib` / `pnpm --dir ui run check` /
  `~/.flexagent/config.toml` / `[agents.*]`
- **関連ドキュメント**:
  - [`docs/04-agent-drivers-and-windows.md`](../04-agent-drivers-and-windows.md)
  - [`docs/05-cli-and-pwa-ui.md`](../05-cli-and-pwa-ui.md)
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md)
  - [`docs/changelog/2026-10-02-acp-enhancement-plan.md`](2026-10-02-acp-enhancement-plan.md)
  - [`docs/changelog/2026-11-08-error-handling-and-recovery-plan.md`](2026-11-08-error-handling-and-recovery-plan.md)
  - [`docs/changelog/2026-11-08-omit-agent-installer.md`](2026-11-08-omit-agent-installer.md)

---

## 0. 結論

`opencode` と `agy` (Google Antigravity) はどちらも ACP の `acp`
サブコマンドを持ち、fxg の ACP クライアントで**すでに会話・ツール・承認・
モデル選択が動く**。日常開発で不足しているのは ACP の土台ではなく、次の 5
領域である。

1. **起動**: `agy` の起動経路がない。`fxg` デーモンが手動起動必須。
2. **認証**: ACP の `authMethods` / `authenticate` を扱えず、未ログイン時の
   案内がない。
3. **入力**: 画像・ファイル添付・`@` 参照を送れない。
4. **復旧**: Revert が会話を戻さない。Fork がネイティブ API を使わず履歴
   Replay の 16k 文字上限に当たる。
5. **診断**: エージェントの stderr と失敗理由を UI / CLI から確認できない。

本計画は、この 5 領域を **Phase 1〜5** に分け、既存の型・API・UI
コンポーネントを再利用しながら実装する。Phase 1 と Phase 2 が「毎日使える」
ための最小セット、Phase 3 が「エージェント TUI と同等の undo/fork」、Phase 4
が「壊れたときに直せる」ための基盤、Phase 5 が仕上げである。

---

## 1. 実測結果 (2026-11-09)

手元の実バイナリに ACP の `initialize` / `session/new`
を直接送って確認した。

| エージェント | バージョン | 確認できた capability                                                                                            | 備考                                                                                                                                                       |
| :----------- | :--------- | :--------------------------------------------------------------------------------------------------------------- | :--------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `opencode`   | 2.2.5      | `loadSession`, `sessionCapabilities.resume/fork/close/delete/list`, `promptCapabilities.image/embeddedContext`   | `authMethods` は `opencode-login` (CLI ログイン)。コマンドは `init` / `review` / `goal` / `new-extension` / `compact`                                      |
| `agy`        | 1.5.3      | `loadSession` / `load`, `sessionCapabilities.resume/close/load/list`, `promptCapabilities.image/embeddedContext` | `authMethods` は Google OAuth / Gemini Enterprise / API key / Agent Platform。モデル・モード (`default` / `accept-edits` / `plan`)・`thought_level` を返す |

両方とも `session/new` は成功し、`configOptions` にモデル選択肢が入る。つまり
**fxg の Composer のモデル / モード選択はこの 2 エージェントで実用になる**。

一方、fxg 側は以下をまだ送っていない / 扱っていない。

- `clientCapabilities.auth` を広告していない (`crates/fxg-acp/src/acp.rs`
  の `ClientCapabilities`)。
- `send_prompt_blocks` を使わず、テキスト 1 ブロックのみ送る。
- `sessionCapabilities.fork` を使わず、常に履歴 Replay で Fork する。
- `initialize` の `auth_methods` を保持せず、`auth_required` を汎用エラーに
  潰している。

---

## 2. ギャップ一覧

| ID  | 優先 | 領域                   | 現状                                                                                                                                     | 実害                                                                          |
| :-- | :--- | :--------------------- | :--------------------------------------------------------------------------------------------------------------------------------------- | :---------------------------------------------------------------------------- |
| G1  | P0   | `agy` 起動             | registry の `antigravity-acp` は `./agy_acp_server.par` を PATH 探索。バイナリ自動導入は削除済みで、`agy acp` の preset / 自動検出もない | `fxg run antigravity` が失敗し、`agy` は custom 定義なしで使えない            |
| G2  | P0   | デーモン起動           | `fxg run` は `fxg daemon` 未起動で即エラー。常駐は `fxg service` を別途設定                                                              | 初回・再起動のたびにつまずく                                                  |
| G3  | P1   | 初回認証               | `authMethods` / `authenticate` / `logout` 未実装。`auth.terminal` 非広告                                                                 | 未ログイン時の `auth_required` が汎用エラー。`fxg agents login` がない        |
| G4  | P1   | 添付                   | `AttachmentMeta` は型だけ。Composer は textarea、`send_prompt` は text のみ                                                              | スクショ・画像・`@` ファイル参照を渡せない                                    |
| G5  | P1   | Revert                 | ファイルは戻すが会話は戻さない。agent は古い文脈のまま                                                                                   | undo 後に文脈とファイルがズレる                                               |
| G6  | P1   | Fork                   | ネイティブ `session/fork` を使わず 16k 文字の履歴 Replay                                                                                 | 長いセッションの fork で文脈が欠落                                            |
| G7  | P1   | 診断                   | agent stderr は debug ログのみ。incidents / health / `fxg doctor` / `fxg logs` 未実装                                                    | 起動失敗・ターン失敗の原因が追えない                                          |
| G8  | P1   | 完了通知               | Web Push は permission / elicitation のみ                                                                                                | 長いターンの完了を待てない                                                    |
| G9  | P2   | Windows プロセスツリー | ACP プロセスは SDK が spawn し、Windows では root のみ kill。fxg の Job Object に載っていない                                            | ラッパー / MCP / LSP の子プロセスが残る可能性                                 |
| G10 | P2   | 子セッション           | opencode の `child-session-updates` や agy の multi-agent 機能で、未登録の ACP session ID からの更新・承認を扱えない                     | subagent / teamwork の承認が cancel される可能性                              |
| G11 | P2   | ツール出力             | `ToolCallContent::Content` / `Terminal` を捨て、`terminal_id` とツールカードを紐付けない                                                 | ツールの詳細が見えない場合がある                                              |
| G12 | P2   | 既定値                 | `session-prefs` は agent + mode のみ                                                                                                     | モデル / thinking level を毎回選ぶ                                            |
| G13 | P2   | CLI                    | TUI 廃止。`session prompt --wait` はポーリングで非ストリーミング                                                                         | ターミナル中心だと物足りない                                                  |
| G14 | P2   | 同時セッション         | warm pool は idle 1 個、1 プロセス 1 セッション                                                                                          | 並列 Worktree で agy のメモリ / 起動時間が増える                              |
| G15 | P2   | 既存セッション取り込み | エージェントが持つ既存セッションを fxg に import する導線がない                                                                          | opencode / agy で始めた会話を fxg で継続できない                              |
| G16 | P1   | 承認ポリシー           | セッション / プロジェクト単位の auto-approve がなく、毎回 Inbox で承認する                                                               | `opencode --auto` / `agy --dangerously-skip-permissions` 相当の運用ができない |

---

## 3. Phase 1: 起動とオンボーディング

### 3.1 G1: 既知 ACP CLI の preset と PATH 自動検出

**問題**

`AcpRegistry::launch_spec` は「custom → registry の `cmd` を `which` →
npx → uvx」の順で解決する (`crates/fxg-acp/src/registry.rs`)。`antigravity` の
alias は `antigravity-acp` に解決され、registry の `cmd` は
`./agy_acp_server.par` である。`clean_cmd` 後は `agy_acp_server.par` という
**PATH 上の名前**になるため、`agy` バイナリは検出されない。

`2026-11-08-omit-agent-installer.md` でバイナリ導入を削除した結果、
この registry エントリは「`.par` を自分で PATH に置いた人」以外は起動できない。
opencode は registry の `cmd` が `./opencode` なので `which("opencode")` に
一致し、そのまま動く。

**設計**

`fxg-acp` に **ビルトイン preset** を追加する。preset は「ユーザーが明示的に
管理しなくても、既知の CLI が PATH にあれば最適な引数で起動できる」ための
組み込み custom 定義であり、新しい配布機構は持たない。

```rust
// crates/fxg-acp/src/registry.rs
struct AgentPreset {
    /// 安定 ID (`fxg run` / `AgentSummary.agent_id` に使う)
    id: &'static str,
    display_name: &'static str,
    /// PATH 上の実行ファイル名 (which / PATHEXT)
    program: &'static str,
    /// ACP サーバーとして起動するための引数
    args: &'static [&'static str],
    /// `fxg run <alias>` で受け付ける別名
    aliases: &'static [&'static str],
}

const PRESETS: &[AgentPreset] = &[
    AgentPreset {
        id: "opencode",
        display_name: "OpenCode",
        program: "opencode",
        args: &["acp"],
        aliases: &[],
    },
    AgentPreset {
        id: "agy",
        display_name: "Antigravity",
        program: "agy",
        args: &["acp"],
        aliases: &["antigravity"],
    },
];
```

- **解決順**: ユーザー custom → preset → registry の外部インストール →
  npx → uvx。preset は registry より優先するが、ユーザー custom が同名なら
  custom を優先する (現行の custom 優先を維持)。
- **alias**: `resolve_alias` で `antigravity` → `agy` に寄せる。
  `antigravity-acp` という registry ID も互換のため残す。
- **AgentSource / 表示**: 新しい enum は追加せず、preset は
  `AgentSource::External` + `registry: false` の「既知 CLI」として返す
  (`AgentSummary.external_path` は解決済みパス)。`installed_agents` にも
  preset 由来の ID を含める。
- **UI**: `ui/src/lib/agents.ts` の `agentSourceLabel` に「既知 CLI」を追加。
  `AgentManager.svelte` のカタログに preset を表示し、`custom` と同様に
  「このノードで利用可能」として扱う。新規セッションの選択肢にも出す。
- **設定での上書き**: `[agents.overrides.<id>]` は追加しない。ユーザーが
  引数を変えたい場合は従来どおり `[agents.custom.<id>]` で上書きする。

**即時のワークアラウンド (コード変更前)**

```toml
# ~/.flexagent/config.toml
[agents.custom.agy]
name = "Antigravity"
command = "agy"
args = ["acp"]
```

`fxg run agy` で起動できる。`antigravity` の名前で呼びたい場合は
`[agents] aliases = { antigravity = "agy" }` を併用する。

**変更ファイル**

- `crates/fxg-acp/src/registry.rs` (preset 定義 / `launch_spec` / `list`)
- `crates/fxg-acp/src/registry.rs` のテスト (fake PATH)
- `crates/fxg-protocol/src/client_api.rs` (`AgentSource::Preset`)
- `crates/fxg-node/src/daemon/mod.rs` (`installed_agents`)
- `ui/src/lib/agents.ts`
- `ui/src/lib/components/settings/AgentManager.svelte`
- `ui/src/lib/components/session/NewSessionChat.svelte`
- `docs/04` §2.1 / `docs/05` §1

**テスト**

- `PATH` に一時ディレクトリで `agy` の stub を置き、
  `launch_spec("antigravity")` が `agy acp` を返すこと。
- ユーザー custom が preset に勝つこと。
- `agy` が PATH にない場合は preset が一覧に出ず、従来どおりエラーになること。
- `cargo test -p fxg-acp --lib` / `cargo test -p fxg-protocol`。

**受け入れ条件**

- `fxg run antigravity` が `agy acp` として起動する。
- `fxg agents list` に `agy` が「既知 CLI (PATH)」として表示される。
- opencode の挙動が変わらない。

### 3.2 G2: デーモンの自動起動

**問題**

`DaemonClient::connect` は IPC 接続に失敗すると「start `fxg daemon`
first」を返す (`crates/fxg-cli/src/client.rs`)。`fxg service install` /
`start` は存在するが、初回の `fxg run` では自動では動かない。Web UI
もデーモンが動いていないと開けない。

**設計**

`fxg-cli` に `ensure_daemon()` を追加し、IPC 接続失敗時に一度だけ自動起動する。

1. `~/.flexagent/daemon.start.lock` を `create_new` で取得する
   (既存ロックが一定時間より古ければ stale として削除)。
2. `fxg service` が install 済みなら `service start` を試す。
3. 未 install なら `current_exe() --daemon`
   をデタッチ起動する。stdio は `~/.flexagent/logs/daemon.log` へリダイレクト。
   - Unix: `setsid` (process group の分離)。
   - Windows: `DETACHED_PROCESS` + `CREATE_NO_WINDOW`。
4. IPC エンドポイントを 100ms 間隔で最大 10 秒待つ。起動したデーモンが
   即終了した場合はログ末尾をエラーに含める。
5. `FXG_NO_AUTO_DAEMON=1` / `--no-daemon` で無効化できる。
6. すべての CLI コマンドで同じ `DaemonClient::connect` を使うため、
   `fxg run` だけでなく `fxg ps` / `fxg session ...` も恩恵を受ける。

**注意**

- 既に `fxg daemon` をフォアグラウンドで起動している場合は、IPC 接続が
  成功するため自動起動しない。
- 二重起動は daemon 側の IPC バインド失敗で検出し、既存デーモンに接続する。
- 自動起動はローカルノードのみ。中央サーバー (`fxg server`) は対象外。

**変更ファイル**

- `crates/fxg-cli/src/client.rs` (`ensure_daemon`)
- `crates/fxg-cli/src/commands.rs` (`DaemonArgs` に `--no-daemon`? もしくは
  環境変数のみ)
- `crates/fxg-cli/src/service.rs` (起動済み判定の再利用)
- `crates/fxg-node/src/ipc_client.rs` (必要なら readiness 用の軽量 ping)
- `docs/05` §1

**テスト**

- 一時 `FXG_HOME` + 空の IPC で `fxg ps` がデーモンを起動して成功する
  (`cli_e2e`)。
- 2 プロセス同時起動でデーモンが 1 つだけになること。
- `FXG_NO_AUTO_DAEMON=1` では従来どおりエラーになること。

**受け入れ条件**

- 新規マシンで `fxg run opencode` が 1 コマンドで動く。
- 失敗時に `daemon.log` のパスが表示される。

### 3.3 G3: エージェント認証

**問題**

`crates/fxg-acp/src/acp.rs` の `initialize` は `fs` / `terminal` /
`elicitation` のみ広告し、`InitializeResponse.auth_methods` を保持しない。
`session/new` が `auth_required` (-32000) を返しても、`build_session` の
`failed to create acp session` に潰れる。opencode は
`opencode auth login`、agy は Google OAuth / Gemini API key を要求する。

**設計**

1. **capability 広告**: `ClientCapabilities` に
   `auth(AuthCapabilities::new().terminal(true))` を追加する。fxg は
   `fxg-pty` でエージェント本体を対話実行できるため、terminal 型を広告して
   よい。
2. **auth methods の保持**: `InitializeResponse.auth_methods` を
   `AgentAuthState { methods: Vec<AuthMethod> }` としてドライバに保存し、
   `ListAgents` / `SystemInfo` ではなく **エージェント起動時
   (`StartSessionRequest`)** に返す。認証はノード上のプロセスに紐づくため、
   グローバルなエージェント一覧に混ぜない。
3. **auth_required の変換**: `session/new` の `Error.code ==
   ErrorCode::AUTH_REQUIRED` を `NodeError::AuthRequired { methods }` に変換し、
   `ErrorCode::AUTH_REQUIRED` (新規) としてクライアントへ返す。
4. **CLI**: `fxg agents login <agent> [--method <id>]` / `fxg agents logout
   <agent>` を追加する。
   - terminal 型: `fxg-pty` で `agy` / `opencode` のログインコマンドを実行し、
     対話入力をそのまま中継する。
   - agent 型: `AuthenticateRequest::new(AuthMethodId)` を送り、完了を待つ。
     URL / device code が返る場合は CLI / UI に表示する。
5. **UI**: `ErrorBanner` の復旧アクションに「ログイン」を追加し、認証方法の
   一覧と実行ボタンを出す。認証中はセッション作成を保留する。
6. **セキュリティ**: 認証情報はエージェントの既存ストア (opencode / Gemini)
   に任せ、fxg は保持しない。ログにトークンを出さない。

**変更ファイル**

- `crates/fxg-acp/src/acp.rs` (`ClientCapabilities` / `initialize` /
  `build_session` のエラー変換)
- `crates/fxg-acp/src/driver.rs` (`AgentAuthState` / `AuthRequired` エラー)
- `crates/fxg-protocol/src/common.rs` (`ErrorCode::AuthRequired`)
- `crates/fxg-node/src/session_manager.rs` (`NodeError::AuthRequired` →
  `ErrorCode`)
- `crates/fxg-cli/src/commands.rs` (`agents login` / `logout`)
- `ui/src/lib/api/errors.ts` / `ErrorBanner.svelte` / i18n
- `docs/04` §2.2 / `docs/troubleshooting.md`

**テスト**

- 擬似 ACP エージェントが `authMethods` を返し、`session/new` で
  `-32000` を返すケースで `AUTH_REQUIRED` + methods がクライアントに届くこと。
- `terminal` 型のログインを stub コマンドで実行できること。
- 実機 (ignored test): 未ログインの `opencode` / `agy` で案内が出ること。
  (既存の認証済み環境を壊さないよう、`FXG_HOME` を分けて検証する)

**受け入れ条件**

- 未認証のエージェントを選ぶと、UI / CLI に「ログイン方法」と実行導線が出る。
- ログイン後に同じセッション作成を再試行できる。

---

## 4. Phase 2: 入力の同等性 (添付 / 画像 / `@` 参照)

### 4.1 問題

- `ActiveSessionHandle::send_prompt(text)` は `String` のみ。
- `SessionManager::send_prompt` は `UserMessage { attachments: Vec::new() }`
  で記録する (`crates/fxg-node/src/session_manager.rs`)。
- `ui/src/lib/components/chat/Composer.svelte` は textarea のみ。
- `fxg run` / `fxg session prompt` に添付フラグがない。
- `AcpSessionHandle` は `ContentBlock::Text` 以外を無視する。

opencode / agy は `promptCapabilities.image` と `embeddedContext` を
広告しているため、fxg 側の実装だけで有効化できる。

### 4.2 プロトコル (DRY)

`text: String` を各メッセージに足していくのではなく、共通の
`PromptInput` を導入する。

```rust
// crates/fxg-protocol/src/common.rs
pub struct PromptInput {
    pub text: String,
    pub attachments: Vec<AttachmentMeta>,
}
```

- `IpcClientMessage::SendPrompt { input: PromptInput, ... }`
- `ClientWsMessage::SendPrompt { input: PromptInput, ... }`
- `ServerToNodeMsg::SendPrompt { input: PromptInput, ... }`
- `CreateSessionRequest.initial_prompt: Option<PromptInput>`
  (`initial_attachments` を別フィールドにしない)
- `UserMessage.attachments` は既存のまま。
- `SessionPromptRequest` など REST 側も `PromptInput` に統一する。

`AttachmentMeta` は既存の `file_name` / `mime_type` / `size_bytes` /
`local_path` / `data_b64` を使う。`local_path` は**ノード側のセッション
ディレクトリ基準の相対パス**に限定し、クライアントが任意の絶対パスを送れない
ようにする。

### 4.3 ACP 変換

`AcpSessionHandle::send_prompt` を `send_prompt_blocks` に変更する。

```rust
let mut blocks = vec![ContentBlock::Text(TextContent::new(input.text))];
for attachment in input.attachments {
    match classify_attachment(&attachment) {
        // 画像は data_b64 を ImageContent へ
        AttachmentKind::Image => blocks.push(ContentBlock::Image(ImageContent::new(
            attachment.data_b64.ok_or(...)?,
            attachment.mime_type,
        ))),
        // テキスト / 小さいファイルは EmbeddedResource へ
        AttachmentKind::Text => blocks.push(ContentBlock::Resource(EmbeddedResource::new(...))),
        // それ以外は ResourceLink (agent が自分で読む)
        AttachmentKind::File => blocks.push(ContentBlock::ResourceLink(ResourceLink::new(
            attachment.file_name,
            file_uri,
        ))),
    }
}
session.send_prompt_blocks(blocks)?;
```

`classify_attachment` は MIME + 拡張子から `Image` / `Text` / `File` を返す
新設のヘルパーとする (`AttachmentMeta` に `kind` フィールドは追加しない)。

- `promptCapabilities.image` がないエージェントには画像を送らない。
  代わりに「画像は添付できないため、パスを本文に含める」フォールバックを
  選ぶか、`ATTACHMENT_UNSUPPORTED` で拒否する。**既定はフォールバック +
  警告**とし、ユーザーの入力を失わせない。
- `embeddedContext` がない場合は `ResourceLink` のみ使う。
- 大きいファイルは `data_b64` にせず `ResourceLink` にする
  (上限は `fxg-node` で 10 MB 程度。設定可能にする)。
- MIME はクライアント申告を信用せず、ノード側で拡張子 / マジックバイトから
  検証する。画像以外の実行ファイルは拒否する。

### 4.4 ノード / CLI / UI

- `fxg-node`: `send_prompt` で添付を検証し、`UserMessage`
  にそのまま記録する。`local_path` は `cwd` 配下に正規化する
  (`dunce::canonicalize` + prefix チェック)。
- `fxg run --attach <path>` / `fxg session prompt --attach <path>` を追加。
  複数指定可。
- Composer: クリップボードの画像貼り付け、ドラッグ & ドロップ、ファイル
  選択、プレビュー、削除、サイズ表示。`@` 入力でノードの
  `GET /api/v1/nodes/:id/fs/browse` を使ったファイル補完を出す。
- タイムライン: `UserMessage.attachments` を画像 / ファイルチップとして表示。
  画像はサムネイル + クリックで拡大。
- 失敗時は入力と添付を保持する (既存の Composer の挙動を維持)。

### 4.5 テスト

- `fxg-protocol`: `PromptInput` の serde / ts-rs 往復。
- `fxg-node`: 相対パス解決、`..` による脱出拒否、サイズ超過、MIME 不一致。
- `fxg-acp`: 添付 → `ContentBlock` 変換の純関数テスト。capability
  なしのフォールバック。
- `ui`: 貼り付け / 削除 / サイズ超過の Vitest。
- E2E: mock ACP agent に画像付き prompt を送り、`session/prompt` の
  content blocks を検証する。

### 4.6 受け入れ条件

- スクショを Composer に貼って送信でき、エージェント側の
  `session/prompt` に `ImageContent` が届く。
- `@` でリポジトリ内ファイルを参照して送信できる。
- 添付不可のエージェントでは警告が出て、本文だけは送信される。

---

## 5. Phase 3: 会話の復旧 (Revert / Fork)

### 5.1 G6: ネイティブ `session/fork`

**現状**

`SessionManager::fork` は常に履歴 Replay を 1 プロンプトとして注入する
(`crates/fxg-node/src/session_manager.rs`)。`MAX_CONTEXT_CHARS = 16_000`
で切るため、長いセッションでは fork 後の文脈が大きく欠落する。opencode /
agy は `sessionCapabilities.fork` を広告している。

ACP SDK 5.0.0 は `unstable_session_fork` feature を有効化済みだが、公開
されている `ConnectionTo::fork_session` は **v2 用**であり、fxg が使う
v1 の `ActiveSession` には fork 用の builder がない。v1 では
`ForkSessionRequest` を直接送り、返った `sessionId` を
`ActiveSession` として attach する必要があるが、`attach_session` は
現在 `pub(crate)` である。したがって実装は次のどちらかになる。

- **上流対応 (第一候補)**: `agent-client-protocol` に v1 用の
  fork builder (または `attach_session` の公開) を追加する PR を出す。
- **fxg 内フォールバック**: `ForkSessionRequest` を `send_request` で送り、
  fork 専用の turn loop (`session/update` を新 session ID でフィルタする
  動的ハンドラ + `session/prompt` の相関) を `AcpDriver` 内に実装する。
  既存の `serve_session` と重複するため、上流対応が入るまでの暫定とする。

**設計**

- `AgentDriver` に fork 用の入力口を追加する。`StartSessionRequest` に
  `fork_from: Option<ForkRequest { agent_session_id, cwd }>` を足すのが
  最小変更。
- `AcpDriver` は `agent_capabilities.session_capabilities.fork` があれば
  v1 の `ForkSessionRequest` を送り、返った session ID を turn loop に
  載せる。上流 builder が入るまでは fork 専用 turn loop を実装する
  (`serve_session` の共通化を検討)。
- 成功時は `context_restored = true` とし、Replay 用の `UserMessage`
  を記録しない。`parent_session_id` / `fork_from_node_seq` は従来どおり
  `SessionCreated` に記録する。
- fork 不可 / 失敗時は既存の Replay にフォールバックし、応答に
  `warnings` を付ける (`FORK_REPLAY_FALLBACK`)。
- `fxg session fork --native/--replay` は不要。既定で native、不可なら
  Replay。UI には「文脈を完全に引き継げない可能性」を警告表示する。

**変更ファイル**

- `crates/fxg-acp/src/driver.rs` (`ForkRequest`)
- `crates/fxg-acp/src/acp.rs` (`build_session` の fork 分岐)
- `crates/fxg-node/src/session_manager.rs` (`fork`)
- `crates/fxg-protocol/src/client_api.rs` (fork 応答の warnings)
- `ui/src/lib/components/chat/ChatTimeline.svelte` / `sessions/[id]`

**テスト**

- mock agent が `sessionCapabilities.fork` を広告した場合に native fork を
  呼び、Replay しないこと。
- fork 非対応 agent では従来どおり Replay し、warning が返ること。
- 実機 (ignored): opencode / agy で長い会話を fork し、直前の文脈が残ること。

### 5.2 G5: Revert で会話も巻き戻す

**現状**

`SessionManager::revert` は `ShadowGitTree::restore` と
`SessionReverted` の記録だけを行う。エージェントの会話はそのままなので、
Revert 後のプロンプトは巻き戻していない会話を前提に応答する。

**設計**

Revert を「ファイル復元 + エージェント会話の再構築」に変更する。

1. 対象 `node_seq` の `snapshot_tree_hash` へファイルを復元する (既存)。
2. 現在のエージェントセッションを停止する (`shutdown`)。
3. 同じ fxg `session_id` のまま、新しいエージェントセッションを開始し、
   `target_node_seq` までの履歴を Replay する。`SessionCreated` は
   再発行せず、`SessionAgentBound` を新しい ACP session ID で更新する。
4. `SessionReverted` に `context_rebuilt: bool` を追加する
   (`#[serde(default)]`)。UI は「ファイルと会話を戻した」と表示する。
5. `--files-only` (CLI) / 「ファイルだけ戻す」 (UI) を用意し、会話の
   再構築に失敗した場合のフォールバックにする。
6. Replay 失敗時はファイル復元を成功として残し、`warnings` で
   「会話は戻せなかった」と返す (`data_safety: workspace_restored,
   context_not_restored`)。

**Replay の改善**

- 上限を 16k 固定から設定可能 (`[agents] replay_max_chars`、既定 64k) に
  する。
- 古い順に落とすのではなく、**直近のターンを優先して残す**。
- ツール呼び出しは生ログではなく「ファイル変更の要約」に圧縮する。
- 切り詰めた場合は `Warnings` に `REPLAY_TRUNCATED` を付ける。

**変更ファイル**

- `crates/fxg-node/src/session_manager.rs` (`revert` / `build_history_replay_context`)
- `crates/fxg-protocol/src/events.rs` (`SessionReverted.context_rebuilt`)
- `crates/fxg-protocol/src/client_api.rs` (`RevertRequest.files_only`,
  応答の warnings)
- `crates/fxg-cli/src/commands.rs` (`session revert --files-only`)
- `ui/src/routes/sessions/[id]/+page.svelte` / `ChatTimeline.svelte`

**テスト**

- revert 後に新しいプロンプトを送ると、巻き戻し後の履歴だけが agent に
  渡ること (mock agent で検証)。
- 再構築失敗時にファイルだけ復元され、warning が返ること。
- 16k 超の履歴で直近ターンが残ること。

### 5.3 補足: `session/close`

セッション終了時に ACP の `session/close` をベストエフォートで送る。
現状は turn loop の終了のみで、エージェント側のセッションが残る。
`acp.rs` の `shutdown` で `CloseSessionRequest` を送り、失敗は debug ログに
留める (pump の終了を妨げない)。

---

## 6. Phase 4: 診断・通知

### 6.1 G7: エージェント stderr と失敗理由の可視化

`error-handling-and-recovery-plan` の Phase 3 (incidents / health /
`fxg doctor` / `fxg logs` / サポートバンドル) を前提とする。本計画では
**ACP エージェント固有の部分**を追加する。

- `crates/fxg-acp/src/acp.rs` の `with_debug` で受けている stderr を、
  プロセスごとのリングバッファ (最後 200 行 / 64 KiB) に保存する。
  `agent-client-protocol` 5.0.0 は非ゼロ終了時に stderr tail をエラーへ
  含めるため、起動失敗時はそれを `StatusChanged(Error)` の
  `details` とインシデントに載せる。
- プロセスは `LaunchKey` 単位で再利用されるため、バッファは
  「プロセスに 1 つ」+ 「現在のセッション ID」の対応で保持する。
  セッション終了時に `session_id` へ紐付けて診断 API から引けるようにする。
- `GET /api/v1/diagnostics` / `GET /api/v1/diagnostics/incidents` に
  `agent_stderr` / `last_acp_error` / `agent_version` / `auth_required`
  を含める。
- `fxg logs --session <id> --follow` / `fxg session diagnose <id>` /
  `fxg doctor` を追加する。
- UI: セッションのエラーバナーに「診断」タブを追加し、stderr tail と
  最後の ACP エラーを表示する。サポートバンドルへ含める場合は
  トークン・Cookie・プロンプト本文を redaction する。

**変更ファイル**

- `crates/fxg-acp/src/acp.rs` (`with_debug` → `StderrRing`)
- `crates/fxg-node/src/session_manager.rs` (セッションへの紐付け)
- `crates/fxg-db` / `fxg-server` (既存 Phase 3 の incidents / diagnostics)
- `crates/fxg-cli/src/commands.rs` (`logs` / `doctor` / `session diagnose`)
- `ui/src/lib/components/ErrorBanner.svelte` / セッション詳細

### 6.2 G8: ターン完了の Web Push

現状の Push は `PermissionRequest` と `ElicitationRequest` のみ
(`crates/fxg-server/src/push.rs`)。`TurnEnded` / `StatusChanged(Idle |
Error)` でも通知する。

- 通知は「実行中だったターンが終わったとき」だけ。`Idle` の連続や
  セッション作成直後は送らない。
- payload は `{ kind: "turn_finished", session_id, title, status }`。
  Approve / Reject ボタンは付けず、タップでセッションを開く。
- 設定で「完了通知」を on/off できるようにする
  (`~/.flexagent/config.toml` の `[push] turn_finished = true` と UI)。
- `hub.rs` のイベント適用後に `notify_turn_finished` を呼ぶ。

**テスト**

- 実行中 → `TurnEnded` で 1 回だけ Push されること。
- 承認待ちの Push と完了 Push が混在しても tag で上書きされないこと。

---

## 7. Phase 5: 仕上げ

### 7.1 G9: Windows の ACP プロセスツリー

`agent-client-protocol` 5.0.0 は Unix では `process_group(0)` +
`kill_process_group` でツリーを終了するが、Windows では
`ChildGuard::terminate` が `Child::kill()` のみで、Job Object を使わない。
ACP SDK が直接 spawn するため、fxg 側から `ProcessTreeGuard` を
attach できない。

- **第一候補**: 上流 `agent-client-protocol` に「spawn hook / Job Object
  連携」を追加する PR を出す。fxg 側は `AcpAgentConfig` に
  `process_tree_guard` 相当のコールバックを渡す。
- **代替**: `fxg agent-exec -- <program> <args>` ヘルパーを追加し、
  - Windows: 自分自身を Job Object に割り当て、子を spawn して stdio を
    中継する。Job のハンドルを保持し、ヘルパー終了時に kill-on-close。
  - Unix: `setsid` してシグナルを中継する。
    ただしラッパーが増えるため、まず上流対応を試す。
- `crates/fxg-acp/src/warm.rs` の「Job Object が回収する」という
  コメントは現状と一致しないため、上流対応まで実態に合わせて修正する。

### 7.2 G10: 子セッション (subagent)

opencode は `_meta.opencode/child-session-updates`、agy は `teamwork` /
`boost` などで子セッションを作る可能性がある。`AcpProcessTaskParams` の
`SessionContexts` は親セッションしか登録しないため、子セッション ID からの
`session/update` / permission / fs / terminal 要求が
「unknown session」になる。

- **短期**: 1 プロセス 1 アクティブセッションという現状を利用し、
  `SessionContexts` に「現在のアクティブセッション」を持たせる。
  未知の session ID の要求は、アクティブセッションが 1 つだけなら
  そのコンテキストへフォールバックし、warning ログを残す。
- **中期**: `_meta` から親子関係を読み、`child_sessions: HashMap<child,
  parent>` を維持する。UI では子セッションの更新を親タイムラインに
  インライン表示する (別セッションとして増やさない)。
- **検証**: 実機で opencode の `task` / agy の `teamwork` を実行し、
  承認が親の Inbox に出ることを確認する。擬似 ACP エージェントで
  子 session ID の permission を返す統合テストも追加する。

### 7.3 G11: ツール出力の完全化

- `tool_call_contents` で `ToolCallContent::Content` (`Text` / `Image` /
  `ResourceLink`) を抽出し、`raw_output` へマップする。
- `ToolCallContent::Terminal` の `terminal_id` を
  `UnifiedEventPayload::ToolCall.terminal_id` として記録し、UI の
  ツールカードからターミナル出力へリンクする。
- `ToolCall` の `terminal_id` 追加は `fxg-protocol` の ts-rs 再生成を伴う。

### 7.4 G12: プロジェクト別の既定設定

`ui/src/lib/session-prefs.ts` は agent + mode のみ保存している。
`config_options` (モデル / thinking level) と `extra_args` も保存し、
新規セッション作成時に適用する。

- プロトコル: `CreateSessionRequest.initial_config: Vec<ConfigOptionValue>`。
- `AcpDriver::build_session` で `session/new` 後に
  `set_config_option` を順に送る。
- UI: 新規セッション画面でモデル / thinking を選べるようにし、
  プロジェクト単位で localStorage に保存する。
- 注意: エージェント側の既定を尊重するため、未設定なら何も送らない。

### 7.5 G13: CLI のストリーミング

`fxg session prompt --wait` は 200ms ポーリングで `session_show` を読む。
IPC に `SubscribeSession { since_seq }` を追加し、イベントを push 配信する。

- `IpcClientMessage::SubscribeSession` /
  `IpcServerMessage::SessionEvent` を追加。
- `fxg session prompt --wait` / `fxg run --wait` は受信した
  `AgentMessage` / `AgentThought` / `ToolCall` を逐次表示する。
- `--json` で構造化出力も選べるようにする。

### 7.6 G14: 同時セッションと warm pool

- `fxg-acp/src/warm.rs` の `DEFAULT_WARM_IDLE` を
  `[agents] warm_idle_secs` で設定可能にする (0 = 無効)。
- 1 プロセス複数セッションの同時ホストは、`2026-10-01-antigravity-startup.md`
  の検証済み設計 (セッションごとの turn loop) を実装する。まずは
  「同一 LaunchKey の同時セッション数を 1 に制限し、2 つ目は新しい
  プロセス」という現状を設定で選べるようにし、必要なら複数ホストへ
  段階的に移行する。
- メモリ上限を超えた場合の eviction 方針 (LRU + TTL) を決める。

### 7.7 `fxg agents install` の意味の整理

バイナリ導入を削除したため、`install` は「npx / uvx の有効化」だけを
行う。コマンド名を `fxg agents enable/disable` に改め、`install` は
非推奨の別名として残さない (AGENTS.md の「過剰な後方互換の排除」)。
`fxg agents list` は `SOURCES` 列で `preset` / `external` / `package` /
`custom` を表示する。

### 7.8 G15: 既存エージェントセッションの取り込み

opencode / agy は `sessionCapabilities.list` / `load` を広告しており、
すでに使い慣れたセッションを fxg で継続できる可能性がある。

- ノード側で `session/list` を呼び、エージェントのセッション
  (`id` / `title` / `cwd` / `updatedAt`) を取得する。
- `cwd` から fxg の論理プロジェクトを解決し、`SessionCreated` 相当の
  投影を作る。新イベント `SessionImported { agent_id, agent_session_id,
  title, cwd }` を追加し、`SessionAgentBound` で既存 ID を紐付ける。
- `fxg session import <agent> [--session <id>]` / UI の「既存セッション」を
  追加する。
- 取り込み時にファイルスナップショットは作れないため、最初の Revert は
  「取り込み後から」になる。UI にその旨を表示する。
- エージェント側の履歴が正であり、fxg のイベントログは取り込み後のみ
  記録する。検索対象は取り込み後の会話に限る。

**変更ファイル**: `crates/fxg-acp/src/acp.rs` (`session/list`)、
`crates/fxg-protocol/src/events.rs` (`SessionImported`)、
`crates/fxg-node/src/session_manager.rs`、`crates/fxg-cli/src/commands.rs`、
`ui/src/routes/+page.svelte` / `NewSessionChat.svelte`。

### 7.9 G16: 承認ポリシー (auto-approve)

`opencode --auto` / `agy --dangerously-skip-permissions` / `accept-edits`
に相当する運用を、fxg でも安全に選べるようにする。

- セッション単位の `PermissionPolicy` を追加する
  (`ask` / `auto_allow_reads` / `auto_allow_all`)。
- 永続化は新しいイベント `SessionPermissionPolicyChanged` と
  `sessions.permission_policy` 投影で行い、全クライアントに同期する。
- `SessionManager` は `request_permission` 受信時にポリシーを評価し、
  対象ならエージェントの `allow_always` (なければ `allow_once`) を自動選択
  して `PermissionResolved { resolved_by: system }`
  を記録する。監査ログ `permission_auto_approved` も残す。
- `auto_allow_all` は明示的な確認 (セッション作成時 / ヘッダーのトグル)
  を必須とし、有効中はヘッダーに常時バッジを出す。
- CLI は `fxg run --auto` / `fxg session control --auto` を追加する。
- ポリシーはエージェントの許可選択肢を超えて権限を付与しない
  (選択肢に `allow` がない場合は `ask` に戻す)。

**変更ファイル**: `crates/fxg-protocol/src/events.rs` /
`client_api.rs`、`crates/fxg-db/migrations/00XX_sessions_permission_policy.sql`、
`crates/fxg-db/src/events.rs`、`crates/fxg-node/src/session_manager.rs`、
`crates/fxg-cli/src/commands.rs`、`ui/.../Composer.svelte` / セッションヘッダー。

---

## 8. 非目標

- ACP バイナリの自動ダウンロード・展開機能の復活。
- 内蔵 TUI (`ratatui`) の復活。Web UI を主界面とする。
- エージェント固有のネイティブ API (opencode の `/undo`、agy の
  内部コマンド) の直接呼び出し。ACP / 汎用 Replay で吸収する。
- マルチユーザー / マルチテナント。
- モデルごとの課金最適化。

---

## 9. 検証計画

### 自動テスト

- `cargo test -p fxg-acp --lib`
  - preset 解決 / 添付 → ContentBlock / fork 分岐 / stderr ring。
- `cargo test -p fxg-protocol`
  - `PromptInput` / `AgentSource::Preset` / `ErrorCode::AuthRequired`
    の serde と ts-rs 契約。
- `cargo test -p fxg-node`
  - 添付パス検証 / revert 再構築 / fork fallback。
- `cargo test -p fxg-cli`
  - `cli_e2e`: daemon 自動起動、`agents login` の stub、`--attach`。
- `pnpm --dir ui run test`
  - 添付 UI、セッション既定値、警告表示。
- `mise run check` (fmt / clippy / Rust / UI)

### 手動検証

1. 空の `FXG_HOME` で `fxg run opencode` → デーモン自動起動 → セッション
   作成。
2. `fxg run antigravity` → `agy acp` 起動。未ログインならログイン導線。
3. スクショを Composer に貼って送信 → エージェントが画像を認識。
4. `@` でファイル参照 → `ResourceLink` / `EmbeddedResource` が届く。
5. 長い会話で Fork → native fork なら文脈維持、非対応なら warning。
6. Revert → ファイルと会話が戻る。`--files-only` も確認。
7. エージェントを強制終了 → セッション詳細に stderr tail と
   診断ボタン。
8. 承認待ちの Push と完了 Push が両方届く。
9. `--auto` / `auto_allow_all` で承認が自動化され、監査ログに残る。
   ヘッダーのバッジと解除も確認。
10. `fxg session import opencode` で既存セッションを取り込み、続きから
    プロンプトを送れる。
11. Windows で `fxg session kill` 後に子プロセスが残らない
    (上流対応後)。

---

## 10. コミット分割案

1. `feat(fxg-acp): add built-in presets and agy PATH detection`
2. `feat(fxg-cli): auto-start the daemon when IPC is unavailable`
3. `feat(fxg-acp): handle ACP authentication methods and auth_required`
4. `refactor(fxg-protocol): introduce PromptInput for prompts`
5. `feat(fxg-acp,fxg-node,ui): send image and file attachments`
6. `feat(fxg-acp,fxg-node): native session fork via session/fork`
7. `feat(fxg-node): rebuild agent context on revert`
8. `feat(fxg-acp,fxg-cli,ui): capture agent stderr in diagnostics`
9. `feat(fxg-server): push turn completion notifications`
10. `feat(fxg-acp,ui): map tool content and terminal links`
11. `feat(ui): persist per-project model and thinking defaults`
12. `feat(fxg-cli): stream session events for prompt --wait`
13. `feat(fxg-protocol,fxg-node,ui): add session permission policy`
14. `feat(fxg-node,ui): import existing agent sessions`
15. `fix(fxg-acp): document the Windows process-tree limitation` /
    `feat(fxg-acp): add a Windows agent-exec wrapper` (上流対応待ち)

各コミットは `mise run check` を通し、`fxg-protocol` の変更時は
`mise run types`、SQL 変更時は `mise run sqlx:prepare` を同じコミットに
含める。

---

## 11. 未確定事項 / 要検証

- **Windows の Job Object**: 上流 SDK に PR を出せるか。出せない場合の
  `agent-exec` ラッパーのオーバーヘッドと stdio 中継の安定性。
- **v1 fork**: `agent-client-protocol` に v1 用 fork builder を追加できるか。
  追加できない場合、fork 専用 turn loop の保守コストを許容できるか。
- **子セッション**: opencode の `task` / agy の `teamwork` / `boost` が
  実際に ACP の子 session ID で permission を送るか。実機で確認する。
- **添付**: opencode / agy が `ResourceLink` の `file://` URI を
  そのまま読めるか。読めない場合は `EmbeddedResource` を既定にする。
- **Revert の再構築コスト**: 長い履歴の Replay はトークンを消費する。
  `--files-only` を既定にすべきか、UI で毎回選ばせるか。
- **モデル既定値**: agy の `models` と `configOptions.model` の
  二重表現をどう正規化するか。`configOptions` を正とする。
- **warm pool**: 1 プロセス複数セッション化した場合のメモリ上限と、
  agy の `_MEI` 相当の一時展開ディレクトリの扱い。
