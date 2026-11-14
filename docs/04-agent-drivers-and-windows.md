# 04. エージェントドライバ実装 (ACP一本化) と Windows 対応詳細

`crates/fxg-acp` および `crates/fxg-pty`
におけるエージェント制御・プロセス管理の具体的な実装設計です。

---

## 1. `AgentDriver` トレイトによる抽象化

すべてのアクション・対話は `AgentDriver` 非同期トレイトで統一します。
型・メソッドの正は `crates/fxg-acp/src/driver.rs` です。

- `AgentDriver::start_session(req, event_tx)` は `StartedSession` を返します。
  `StartedSession` は操作ハンドル、`context_restored`、`agent_session_id` を持ちます。
- `ActiveSessionHandle` は `send_prompt(PromptInput)`、`respond_permission`、
  `respond_elicitation`、`set_mode`、`set_config`、`cancel_turn`、`dispose`、
  `shutdown` を提供します。
- `dispose` はプロセスツリーを含めて終了し、`shutdown` はセッションを閉じて
  プロセスを再利用プールへ返します。
- `DriverKind` は `Acp` / `AcpExtended` / `Custom` / `Other` です。

### 1.1 エージェント実装の3層モデル

ACP だけでは、アイコンや起動オプションなどのエージェント固有メタデータや、
ACP に存在しない機能（独自 API によるモデル名取得など）を扱えない。将来の
拡張を `agent_id` 分岐なしで追加できるよう、実装を次の3層に分ける。
現在は `AgentCatalog`、Tier 2 (`ExtendedAcpDriver` + `AgentExtension`)、
Tier 3 (`impl AgentDriver`) の基盤を持ち、`opencode` は Tier 2 で実装している。

| 層                   | 実行時の実装                           | ACP の再利用 | エージェントごとに追加するコード            |
| :------------------- | :------------------------------------- | :----------- | :------------------------------------------ |
| Tier 1: ACP only     | `AcpDriver`                            | すべて       | なし（registry / preset / custom 定義のみ） |
| Tier 2: ACP + custom | `ExtendedAcpDriver` + `AgentExtension` | すべて       | `AgentExtension` の差分のみ                 |
| Tier 3: Full custom  | `impl AgentDriver`                     | なし         | 起動・イベント変換を含む driver 実装        |

- エージェント固有の知識は `AgentCatalog` と extension に閉じ込め、
  `SessionManager` と `AcpDriver` には追加しない。
- 実行時カタログは **custom 定義 → builtin provider** の順で解決する。
  ACP Registry は実行時カタログには含めず、custom 定義を import するための
  テンプレートとしてだけ使う（§2.1・§3.1）。
- `AgentLaunchSpec` は program / args / env / driver_kind / extension を持つ
  純粋データとし、warm pool のキー（`LaunchKey`）として扱う。extension の
  インスタンスは含めない。
- Tier 2 は `ExtendedAcpDriver` が ACP イベントを中継し、
  `CapabilitiesUpdated` に `CapabilityOverlay` を適用してから
  `SessionExtension` に通す。capability probe も同じ経路を通る。
  overlay はイベントごとに現在値（`CapabilitiesSnapshot`）から再計算する。
- ユーザー定義のカスタム ACP エージェント（§3）は既定で Tier 1。
  `CustomAgentConfig.extension` に builtin extension ID を指定すると
  `DriverKind::AcpExtended` となり、同じ overlay / メタデータを再利用できる。
- driver の選択は `AgentLaunchSpec.driver_kind` と `extension` で行う。
  Tier 2 の `ExtendedAcpDriver` は extension ID ごとに1つだけ生成して共有する。
- Tier 3 の builtin ID は予約し、custom 定義による上書きを拒否する。
- 組み込みの `opencode` extension は、ACP が返す `provider/model` を
  `{ value, name, providerID, id }` に変換して読みやすいモデル名を表示する。
  `value` は元の ACP ID のまま保持するため `set_config` は従来どおり動作する。
- `AgentSummary.icon` は `builtin:<id>` または URL を返す。Web UI は
  `builtin:opencode` を同梱アイコン (`/agents/opencode.png`) に解決し、
  `builtin:antigravity` / `builtin:copilot` / `builtin:qwen` /
  `builtin:cline` / `builtin:auggie` / `builtin:kimi` を
  インライン SVG（`currentColor`）で描画する。未知のキーや未指定は汎用の
  bot アイコンにフォールバックする。
  `AgentSummary.backend` はエージェント一覧にバッジ
  (`ACP` / `ACP + extension` / `Custom driver`) として表示する。
- ビルトイン provider は `opencode`（Tier 2）に加え、
  `agy` / `github-copilot-cli` / `qwen-code` / `cline` / `auggie` /
  `kimi-code` のネイティブ CLI preset を持つ。一覧・起動は
  ビルトインの表示名・アイコン・preset を使う。opencode は v2 の CLI 名が
  `opencode` であるため、`opencode-v2` のような別 ID を持たない。
- Gemini CLI は上流で deprecated のため builtin / alias を持たない。
  必要な場合は Registry から import して custom 定義として使う。
- builtin にないエージェントは、Web UI の「Import from registry」で
  ACP Registry の配布レシピを custom 定義として保存する。import した
  custom は通常の custom として編集・削除できる。
- ACP 接続の縁に必要な差分（`initialize` の
  `clientCapabilities._meta`、受信する独自通知の method、有効にする ACP
  capability の集合）は、`AgentExtension` が純データ
  (`AcpExtensionOptions`) として返す。`AcpDriver` は通知の受信・順序・
  子セッションのコンテキスト管理・正規化だけを行い、opencode 固有の
  解釈は `OpenCodeExtension` に閉じる。`AcpDriver` にはエージェント別の分岐を
  入れず、汎用の ACP seam だけをコアに置く。
- ビルトイン provider は拡張を `with_extension(<extension ID>, <instance>)`
  で明示する。extension ID はエージェント ID から暗黙導出しない。カスタム定義は
  この ID を `CustomAgentConfig.extension` に保存し、一覧
  (`AgentSummary.extension`) にも表示する。
- ACP capability の広告は接続ごとに決まる。ビルトインは
  `AgentAcpCapabilities::default()`（`subagents` ON）を読み取り専用で使い、
  カスタム定義だけが `CustomAgentConfig.capabilities` として自分の選択を持つ。
  値は起動スペック (`AgentLaunchSpec.acp`) に適用され、warm pool のキーにも
  含まれるため、設定変更後に古い capability で初期化済みのプロセスを
  再利用しない。UI は custom agent の編集フォームで変更し、一覧では全員の
  有効な capability をバッジで表示する。

---

## 2. 任意ACPエージェント対応 (`AcpDriver` & Registry Manager)

### 2.1 ACP Registry の import とローカル起動解決

1. **レジストリの役割 (import 専用)**:
   - 公式インデックス
     `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`
     を取得し、`<cache>/node/registry.json` にキャッシュする（TTL
     24時間、Web UI のカタログ更新で即時更新）。
   - レジストリは **実行時カタログには含めない**。Web UI の
     「Import from registry」で custom 定義を生成するためのテンプレートとして
     だけ使う。
   - `GET /api/v1/agents` は `agents`（builtin + custom）と `registry`
     （import 候補）を返す。レジストリの取得に失敗、または5秒でタイムアウト
     しても `agents` は返す。
2. **起動解決の優先順位 (ローカル優先)**:
   1. カスタム定義 (`node.db` の `custom_agents`)
   2. Tier 3 の full custom provider
   3. **ビルトイン preset**: 既知の CLI が `PATH` にあれば、その ACP
      起動引数で起動する (`opencode acp` / `agy acp`)。ユーザーが同名の
      custom を定義した場合は custom が優先される。
   - Registry のエージェントを直接起動することはない。`npx` / `uvx` で
     使いたい場合は、先に import して custom 定義にする。
   - 代表例: `fxg session new opencode` は `opencode acp`、`fxg session new agy`
     は `agy acp` として起動する（opencode は Tier 2 としてモデル名を補う）。
3. **import 時のコマンド変換**:
   - `binary`: `PATH` 上の CLI が見つかればそのコマンドを使う。`PATH` に無く
     npx / uvx 配布があれば、その配布形態で起動する。binary のみで `PATH` に
     無い場合は import 不可にする。
   - `npx`: `command = npx`, `args = ["-y", <package>, ...]`。
   - `uvx`: `command = uvx`, `args = [<package>, ...]`。
   - Registry の `description` / `icon` を custom にコピーし、`version` は
     `origin = registry { id, version }` として保存する。
4. **インストールは明示操作のみ**:
   - fxg はバイナリ配布のダウンロード・展開を行わない。binary 配布のみの
     エージェントは、各ツールの配布元から導入して `PATH` に置く。
   - import した custom はスナップショットである。Registry の更新は自動反映
     しない。更新する場合は custom を削除して再 import する。

### 2.2 `agent-client-protocol` クレートを用いた `Client` 実装

ACPでは、エディタやオーケストレータ側が **`acp::Client` トレイト**
を実装してサブプロセスの `stdin/stdout` に接続します。

`fxg-acp` の ACP client handlers が処理する主要コールバック：

- **`session_update(notification)`**:
  - `AgentMessageChunk`, `AgentThoughtChunk` ➔
    リアルタイム配信チャネルへ即時送出しつつ、バッファに蓄積。
  - `ToolCall`, `ToolCallUpdate` ➔ `locations` や `content` (Diff) を抽出し
    `UnifiedEventPayload::ToolCall` として `node.db` へ記録。
  - `AvailableCommandsUpdate` ➔
    スラッシュコマンド一覧を更新し、UIとCLIの補完リストに反映。
  - `CurrentModeUpdate` / `ConfigOptionUpdate` ➔
    現在のモードやモデル設定選択肢をUIへ同期。
    ACP は Session Modes を Session Config Options に置き換えつつあり、
    OpenCode のように `modes` を返さず `category = "mode"` の
    `configOption` だけでモードを提示するエージェントが存在する。
    `fxg-acp` は両方を同じ `ModeInfo` / `current_mode` に正規化し
    (`session_capabilities`)、`set_mode` はモード用 `configOption` があれば
    `session/set_config_option`、無ければ従来の `session/set_mode` に振り分ける。
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
  - form モードのみを広告する
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

### 2.3 サブエージェントのルーティング

ACP のセッション更新は `sessionId` を持つが、`agent-client-protocol` の
セッションルーターは **root セッション宛の通知だけ** を `serve_session` に
流す。子セッション宛の通知は通常の経路では捨てられるため、fxg は次のように
扱う。

- root 宛の `session/update` は従来どおり `ActiveSession` が受ける。
- root 宛の更新はターン終了後も読み続ける。エージェントによってはプロンプト
  応答の後にバックグラウンドの報告を送る（agy の teamwork など）ため、
  アイドル中も `read_update` を select に含める。`PromptResponse` が無いので、
  最後の更新から一定時間（`IDLE_FLUSH_DELAY`）静かになった時点で結合中の
  ストリームを完了イベントとして確定する。次のプロンプト開始時にも保留分を
  確定してから送る。
- `user_message_chunk`（ターン先頭のユーザーメッセージ echo）を境界として、
  それ以前に蓄積した root のストリームを確定する。agy のように「次の
  プロンプトが来るまでバックグラウンドの報告を送らない」エージェントでは、
  報告が新ターンの回答と連結されるのを防ぐ。
- 子セッション宛の `session/update` は、`subagent_update` で子を知った時点で
  子ごとの dynamic handler を登録して受ける。ハンドラは子の ID 以外を
  `Handled::No` にして root の経路を壊さない。
- opencode の `opencode/session/child_update` のような独自通知は、extension が
  宣言した method だけを method handler で受ける。builder の catch-all は
  root の `session/update` を奪うため使わない。
- 受けた子イベントは親と同じ accumulation（ストリーミング結合）に合流させ、
  `subagent_id` を付けて正規化する。子の状態は `SubagentUpdated` で表し、
  親の `StatusChanged` や `sessions` 投影を書き換えない。
- `SessionContexts` には子の `cwd` を登録し、子からの fs / terminal /
  permission 要求を子のコンテキストで解決する。子を告知しないエージェントの
  ために「登録済みコンテキストが 1 つだけなら親へフォールバック」も残す。

`initialize` で広告する client capability はエージェント定義ごとの
`AcpExtensionOptions` が持ち、`AcpCapabilitySet` として未知の名前を捨てずに
保持する。ACP 標準の `subagents` は既定で ON（unstable RFD）。opencode は
`_meta` の vendor capability も広告する。どちらも有効化すると、子は独立
セッションにはならず、親タイムラインのサブエージェントブロック / 詳細ビュー
として表示される。

ACP の必要な unstable feature（`unstable_session_fork` /
`unstable_subagents`）は `agent-client-protocol` の依存で常に有効化する。
ビルド時に分離したい場合だけ Cargo feature を追加する。広告する capability は
起動ごとの `AcpCapability` / `AcpCapabilitySet` で決め、プロセス全体の環境変数
では上書きしない。将来 ACP の capability を増やすときは variant と mapping を
追加する。エージェントごとの分岐は extension にのみ置く。

---

## 3. カスタム ACP エージェント (任意コマンド)

ACP Registry に載っていないエージェントや自前の ACP 実装は、**明示的なカスタム
定義**として登録して起動する。

1. **定義方法**:
   - Web UI (設定 > エージェント管理 > カスタム追加) または API から
     `node.db` の `custom_agents` テーブルへ保存する (`name` / `description` /
     `command` / `args` / `env` / `env_secrets` / `extension` / `icon` /
     `origin`)。
   - API: `PUT /api/v1/nodes/:node_id/agents/custom/:agent_id` (upsert) /
     `DELETE /api/v1/nodes/:node_id/agents/custom/:agent_id`。
   - 実行時設定はノードの DB にのみ保存し、ハブへ同期しない。
2. **解決と起動**:
   - 既定では Tier 1 (`AcpDriver`) として解決する（§1.1）。`extension` を
     指定した場合は Tier 2 (`ExtendedAcpDriver`) になる。
   - `command` は `PATH` 上のコマンド名または絶対パス。シェル経由ではないため
     グロブ・`~`・環境変数の展開は行わない。
   - 起動直前に `fxg_pty::resolve_command` が `which` (`PATHEXT`) で解決し、
     `args` / `env` を付けて ACP プロセスとしてスポーンする。
3. **Registry からの import**:
   - `POST /api/v1/nodes/:node_id/agents/registry/:agent_id/import` で、
     Registry の配布レシピを custom 定義として保存する。
   - `command` / `args` / `env` はノード上で解決した配布形態（binary / npx /
     uvx）から生成し、`description` / `icon` / `origin` もコピーする。
   - 既存 custom、Tier 3 の予約 ID、未知の extension は拒否する。
   - import 後は通常の custom として編集・削除できる。Registry の更新は自動
     反映しない。
4. **Tier 2 extension の再利用**:
   - `CustomAgentConfig.extension` で builtin extension を参照できる（§1.1）。
     指定時は `DriverKind::AcpExtended` となり、custom の起動定義に extension の
     `prepare_launch` を適用する。組み込みの `opencode` extension はモデル名を
     読みやすい表示へ変換する。
   - 未知の extension ID は保存時と起動時に拒否する。
5. **一覧表示と優先順位**:
   - カスタム定義は常に「利用可能」として扱い、`sources: ["custom"]` で返す。
     同じ ID の builtin より優先し、重複表示しない。
   - extension を参照するカスタム定義は `backend: "acp_extended"` として返す。
   - Tier 3 の builtin ID は予約し、custom 定義による上書きを拒否する。

---

## 4. セッションの Fork（ACPネイティブのみ）

**Fork** は、ACP の `session/fork` + `session/load` をそのまま使う場合にのみ提供します。
fxg 側で履歴を組み立て直す Replay フォールバックは行いません。

- 対象は **同じエージェント・同じノード上にあるセッション** のみです。現在のセッション全体を
  分岐し、新しい `session_id` を作成します。任意のメッセージ地点を指定した Fork は
  ACP に存在しないため、現時点では提供しません。
- エージェントが `sessionCapabilities.fork` と `loadSession` の両方を広告した場合だけ
  実行できます。未対応の場合は `FORK_UNSUPPORTED` を返し、Web UI では Fork ボタンを
  表示しません（`CapabilitiesUpdated.fork_scope` を `sessions.fork_scope` に投影）。
- `fork_scope` は `none` / `last` / `message` の3値です。ACP ネイティブの
  `session/fork` は現在の会話末尾しか分岐できないため、現行実装は `last` を広告します。
  `message` は将来の ACP 拡張（メッセージ境界の指定）に対応したエージェントだけが
  広告できます。
- `SessionForkSpec.up_to_node_seq` は Fork 元の `node_seq` を指定します。`last` の
  セッションでは末尾以外の値を指定すると `FORK_SCOPE_UNSUPPORTED` を返します。
  `message` 対応エージェントでは、この値までを Fork 先のイベントログに複製します。
- 新しいセッションは `parent_session_id` を系譜として記録し、Fork 元の会話イベント
  （ユーザー／エージェントメッセージ、思考、ツール呼び出し、計画、ターミナル出力、
  サブエージェント）を新しいイベントログへ複製します。会話の実体はエージェントが
  保持しており、この複製は Web UI / CLI が継承した履歴を表示するためのものです。
  セッション固有のメタイベント（作成・タイトル・状態・能力・承認・利用量）は
  複製しません。複製範囲は Fork 作成時点の `node_seq`
  までに限定します。
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

### 4.2 承認ポリシー (`PermissionPolicy`)

セッション単位で `ask` / `auto_allow_all` を切り替えられます。
`auto_allow_all` では、エージェントが `request_permission` で提示した
`allow_*` オプション (`allow_always` を優先) をノードが自動選択し、
`PermissionResolved` を `resolved_by = "auto"` として記録します。
ポリシーは `SessionPermissionPolicyChanged` イベントで
`sessions.permission_policy` に投影され、Web UI の Composer の
`Auto` トグルから変更できます。

ポリシーの変更はエージェント (ドライバ) を必要としないため、停止中
(未レジューム) のセッションでも受け付けます。変更は `sessions` に
永続化され、レジューム時にはその値が `ActiveSession` に復元されます
(クライアントの表示と実際の自動承認動作を一致させるため)。

### 4.3 プロジェクト別の初期設定

Web UI はプロジェクトとエージェントの組み合わせごとに最後に選択した
`configOptions` (モデル / thinking level 等) を localStorage に保存し、
新規セッション作成時に `CreateSessionRequest.initial_config` として送ります。
ノードは `session/new` 後に `session/set_config_option` を順に送って適用します。

モデル値をエージェントごとに分けるのは、値の形式がエージェント固有だからです。
たとえば OpenCode は `provider/model` を要求し、`provider/` の付かない ID は
`session/set_config_option` で拒否されます。他エージェントの値を混ぜると、
能力プローブの `initial_config` が失敗し、適用できないモデルが残ります。
`initial_config` には、現在のエージェントのプローブ結果と照合済みの値だけを
送ります (プローブできなかった場合はエージェント既定値を使う)。

### 4.4 新規セッション前の能力プローブ

ACP は `initialize` ではモデル一覧を返さず、`session/new` のレスポンスに含まれる
`configOptions` が唯一の情報源です。そこで Web UI は
`POST /api/v1/nodes/:node_id/agents/:agent_id/probe` を呼び、ノードが
**一時的な ACP セッション**を1回だけ開始して能力を取得します。

- **DB 非汚染**: プローブは `SessionManager::probe_agent_capabilities` が
  driver を直接使い、`sessions` 行やイベントを記録しない。
- **ウォームプール再利用**: 既存の `AcpDriver` / warm pool を使うため、プローブで
  起動したプロセスを実セッションが再利用でき、初回起動コストを相殺できる。
- **キャッシュ**: `(agent_id, local_path, initial_config)` 単位で約10分保持する。実セッションの
  `CapabilitiesUpdated` でもキャッシュを更新する。同時プローブは直列化する。
  UI の「Refresh」は `force: true` でキャッシュを迂回する。
- **失敗時**: `AUTH_REQUIRED` は通常のログイン復旧導線へ流す。タイムアウトや
  起動失敗時も新規セッション自体は開始でき、モデルはエージェント既定値を使う。
- **制約**: 一時VMはノード起動前なのでプローブできない。また、エージェント側に
  空のセッションが残る実装がある (ACP にセッションを作らない一覧 API はない)。
- **新規セッション UI**: モデル pill は常に表示し、取得中は `Default` のまま開始を
  ブロックしない。初期モードの選択肢はプローブ結果の `available_modes`
  (ACP `modes` と `category = "mode"` の `configOption` を正規化したもの) から
  生成する。モード ID はエージェント固有のため、プローブ前は推測の ID を出さず
  `Default` のみを表示し、エージェントを変更したときは選択を `Default` に戻す。
  `category = "mode"` の `configOption` はモード専用の pill に投影されるため、
  モデル pill 列には出さない。

### 4.5 外部セッションの発見と取り込み（手動同期）

ACP `session/list`（安定）を使い、fxg がまだ管理していないエージェント側の
セッションを一覧し、明示操作で取り込む。方針は **「発見は自動相当（読み取り
専用）、取り込みは明示」** であり、バックグラウンドで勝手に fxg セッションを
作らない。

- **発見**: ノードはエージェントを短命プロセスで起動し、`initialize` の
  `sessionCapabilities.list` を確認して `session/list` をカーソルでページング
  する。`session/new` は作らない。結果は短時間キャッシュし、UI の「今すぐ
  同期」はキャッシュを迂回する。エージェントごとにタイムアウトと全体予算を
  設け、1つの失敗で全体を落とさず `agents[].error` として返す。
- **取り込み**: 一覧には `imported_session_id`（既存 fxg セッション）を付け、
  UI は「開く」を出す。未取り込みのみ `POST /api/v1/sessions/import` を呼ぶ。
  外部 CLI が同じセッションを使用中の二重クライアントを避けるため、自動では
  取り込まない。
- **履歴ミラー**: `mirror_history: true` は `session/resume` より
  `session/load` を優先する。`session/load` の replay 通知は live と同じ
  正規化経路へ流し、`Import` モードでのみ replay のユーザーメッセージを
  `client_source = "import"` の `UserMessage` として生成する（live ターンは
  従来どおり manager が記録する）。`loadSession` 非対応、または
  `session/load` が成功しても履歴を 1 件も replay しなかった場合は
  `HISTORY_UNAVAILABLE` を返し、空のセッションを黙って作らない。クライアントは
  履歴なしの取り込みに切り替える。
- **重複排除とロールバック**: 生きている `(agent_id, agent_session_id)` を
  重複とみなす。取り込みの起動に失敗した行は `SessionDeleted` でロールバックし、
  幽霊セッションを残さない。
- **制約**: `session/list` 非対応のエージェントは ACP 経由では発見できない。
  各ツールのローカル保存を直接読むフォールバックは持たない。replay には
  メッセージ単位のタイムスタンプが無いため、取り込んだイベントの時刻は
  取り込み時刻になる（表示順は `node_seq` で保たれる）。

## 5. Windows サポートと PTY 双方向制御 (`crates/fxg-pty`)

### 5.1 Windows Job Object によるプロセスツリー完全終了

Windowsでは `node.exe` や `.cmd` ラッパー経由で起動したプロセスを
`Child::kill()`
しても、孫プロセス（言語サーバーやテストランナー、開発サーバー）が生き残りファイルをロックする問題が多発します。
これを防ぐため、fxg が直接起動するターミナル（PTY）とプロビジョナーの子プロセスを
**Windows Job Object** にバインドします。ACP エージェントプロセスは
`agent-client-protocol` が起動し、Unix ではプロセスグループ、Windows では
`CREATE_NO_WINDOW` を使用します。Job Object の対象は PTY とプロビジョナーです。
実装は `crates/fxg-pty/src/proc.rs` の `WinJobGuard` / `ProcessTreeGuard` です。
Job Object は `kill_on_job_close` を設定し、起動したプロセスを `assign_process` で
割り当てます。

### 5.2 コマンド解決 (`which` + `PATHEXT`) とパス正規化 (`dunce`)

- **コマンド解決**: `npx`, `uvx`, `claude` などを起動する際、必ず
  `fxg_pty::resolve_command` が `which::which_in(cmd, env::var_os("PATH"), &cwd)`
  を通すことで、`claude.cmd` や `npx.cmd` の拡張子を確実に解決します。
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
- **ストリーミング**: PTY の Master `Read` を専用の読み取りスレッドで読み取り
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
ノードまたはハブを登録します（既定は `node`）。`service` は **OS への登録・解除**を担当します。
Linux / macOS は登録時に起動し、Windows は次回ログオン時に起動します。
起動・停止・再起動は `fxg node start|stop|restart` / `fxg hub start|stop|restart`
が行い、unit が登録済みの場合は systemd / launchd / Task Scheduler に委譲します。
`fxg node status` / `fxg hub status` は service 登録と HTTP の状態を表示します。

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
状態確認と操作ができます（既定は無効）。常時有効化はノードの Web UI
（設定 > ノード > 一般の Tray）または Client API で `node.db` の
`node_settings.tray` を `true` にするか、`fxg node start --tray` を使います。
`--no-tray` は起動時の一時的な無効化です。`fxg internal stdio`
（一時VM・プロビジョナー）では常に無効です。

- **メニュー**:
  - ステータス表示: ハブ接続状態（●接続中 / ○スタンドアロン）、
    未同期 Outbox 件数、稼働中セッション数（3秒間隔で更新）
  - 「ログイン時に自動起動」: `fxg service install` / `uninstall` と同一実装の
    チェック項目（登録後は実状態を再確認して表示を同期）
  - 「Web UI を開く」: `fxg web` と同じトークン付き URL を既定ブラウザで開く
  - 「終了」: ノードの graceful shutdown（エージェント/PTY 停止 →
    `nodes.is_online = 0` 記録）を実行して終了
- **アイコン**: `ui/static/icon-192.png` を 32×32 へ縮小し、右下に接続状態ドット
  （緑=ハブ接続中 / グレー=スタンドアロン）を合成する。
- **状態の取得元**: プロセス内のノード状態（`node.db` / 同期ワーカー）を直接
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
