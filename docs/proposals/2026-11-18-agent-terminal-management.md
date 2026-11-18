# 2026-11-18 エージェント端末の管理（所有権・表示・停止）

- **日付**: 2026-11-18
- **ステータス**: `Partially Implemented`
- **対象パッケージ**: `fxg-protocol`, `fxg-agent`, `fxg-node`, `fxg-pty`, `ui`
- **対象スクリプト / 設定**: `mise run check` / `mise run fmt` / `mise run types`
- **関連ドキュメント**:
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md) §1.2 (永続化の境界), §1.4 (制御アクション)
  - [`docs/04-agent-drivers-and-windows.md`](../04-agent-drivers-and-windows.md) §2.2 (`terminal/*`), §5.3 (`PtySessionManager`)
  - [`docs/07-web-ui-and-pwa.md`](../07-web-ui-and-pwa.md) §3.2 (チャットタイムライン), §4 (Web ターミナル)
  - [`docs/proposals/2026-11-14-terminal-workspace-scope.md`](./2026-11-14-terminal-workspace-scope.md) (workspace 端末の寿命)
  - [`docs/proposals/2026-11-14-opencode-native-driver.md`](./2026-11-14-opencode-native-driver.md) (Tier 3 ドライバ)
  - [`docs/changelog/2026-11/2026-11-18-agent-terminal-cards.md`](../changelog/2026-11/2026-11-18-agent-terminal-cards.md)

---

## 0. 結論

- **端末の所有権は「プロセスを起動した主体」**とする。fxg が kill・入力・
  スナップショットできるのは、fxg が PTY を持つ端末だけ。エージェント内部で
  起動されたコマンドは、表示はできても個別制御はできない。
- ACP の `terminal/*` は **opt-in の Client capability** であり、委任は
  エージェント次第。fxg は v1 の `terminal` を広告しているため、委任する
  エージェントは自動的に fxg の PTY を使う。自前の bash ツールを使う
  エージェントはツールカード（`ToolCall`）になるだけ。
- ACP v1 の `terminal/*` に**クライアントからの標準入力は無い**。対話操作は
  統合ターミナル（fxg 自身の PTY）で行う。
- ACP v2 ではクライアント側 terminal 面が削除される予定で、`kill_terminal` は
  v1 限定の機能になる。将来の共通解は **fxg が MCP の端末サーバーを公開する**
  方向とする（§4-B）。
- OpenCode ネイティブは現状、個別のコマンド制御ができない（`interrupt` のみ）。
  OpenCode 側の experimental な persistent PTY を読み取り専用で接続する案を
  補助的に位置づける（§4-C）。
- 2026-11-18 時点で実装済みなのは §6 Phase 0（ACP v1 端末のライフサイクル・
  単体停止・ターン終了スナップショット・UI・Windows 終了経路）。

## 1. 現状 (2026-11-18)

fxg には用途の異なる 3 系統の端末がある。混同すると「なぜこのコマンドは
止められないのか」が説明できなくなるため、所有者を明確に分ける。

| 系統                                | 誰が起動するか     | プロセスの所有者 | 表示         | 単体停止                | 入力                     |
| :---------------------------------- | :----------------- | :--------------- | :----------- | :---------------------- | :----------------------- |
| 統合ターミナル (workspace PTY)      | ユーザー           | fxg (ノード)     | 端末そのもの | 可 (タブを閉じる)       | 可 (Ctrl-C 等)           |
| ACP クライアント端末 (`terminal/*`) | エージェントが要求 | **fxg (ノード)** | 端末カード   | 可 (`kill_terminal`)    | 不可 (v1 に method 無し) |
| エージェント自前実行                | エージェント       | エージェント     | ツールカード | 不可 (ターン中断のみ)   | 不可                     |
| OpenCode ネイティブ                 | OpenCode サーバー  | OpenCode         | ツールカード | 不可 (`interrupt` のみ) | 不可                     |

### 1.1 ACP v1 で実装済みの範囲

- `initialize` で `clientCapabilities.terminal = true` を広告する
  (`fxg-agent/src/acp/session/capabilities.rs`)。`fs` と `auth.terminal` も同様。
- `terminal/create` で `fxg-pty` を起動し、開始イベント・出力のライブ配信・
  ターン終了時スナップショット・終了イベントを `TerminalOutput` として記録する
  (`docs/03` §1.2)。
- `SessionControlAction::KillTerminal` で 1 つの端末だけを終了する
  (`docs/03` §1.4)。UI の端末カードは実行中だけ停止ボタンを出す。
- `terminal/output` / `wait_for_exit` / `kill` / `release` は ACP の要求どおり
  実装している。`release` は kill も行う。

### 1.2 ACP v1 の制約

- **入力**: `terminal/*` に「クライアントから stdin を書く」method が無い。
  端末カードは表示専用で、対話コマンドは統合ターミナルで実行するしかない。
- **出力の所有**: 出力はクライアントが保持する。エージェントが
  `terminal/output` を呼ばなければ、モデルには何も渡らない。
- **ツールカードとの関係**: ACP には `ToolCallContent::Terminal`（ツール
  コールに端末を埋め込む）があるが、fxg はまだ `tool_call_id` と
  `terminal_id` を関連付けていない（[`docs/backlog.md`](../backlog.md) §4）。

### 1.3 OpenCode ネイティブの現状

- `fxg-agent/src/opencode/client.rs` は session の prompt / command /
  interrupt / compact / permission / form / revert のみを使う。ツールは
  OpenCode サーバー内で実行され、fxg は `tool_call` の status と
  `raw_output` を受け取るだけ。
- したがって表示（execute ツールカード、出力プレビュー、exit code）はできるが、
  PID を持たないため個別 kill も stdin 注入もできない。

## 2. 問題

1. **UX の不統一**: 同じ「エージェントがコマンドを実行した」結果が、委任の
   有無で端末カードとツールカードに分かれる。停止できるかどうかも変わる。
2. **自前実行の扱い**: 長時間コマンドでも `interrupt` 以外に止める手段が無く、
   止めたときに子プロセスが残るかはエージェント依存。
3. **ACP v2 への移行**: v1 のクライアント端末は削除予定で、`kill_terminal` の
   土台が無くなる。v2 の Agent-owned 表示専用ストリームでは停止はできない。
4. **対話コマンド**: ACP v1 には入力経路が無く、UI 上は「表示はできるが操作
   できない端末」になる。
5. **端末の重複**: 統合ターミナルと OpenCode の PTY、ACP 端末が別系統で、
   「どの端末がどのコマンドか」を横断して見る手段が無い。

## 3. 設計方針

### 3.1 不変条件

1. **所有権 = spawn した主体**。fxg は自分が PTY を持つ端末だけを kill・入力
   できる。他者のプロセスを PID 推測で kill しない。
2. UI は所有権を表示で区別する（端末カード / ツールカード）。停止操作は
   「fxg 所有なら単体停止、それ以外はターン中断」を明示する。
3. 端末のライフサイクルはターンと独立する。ターン終了で kill しない。
4. 端末の出力は「開始・差分・終了」のイベントで永続化し、リロードで復元できる
   (実装済み、`docs/03` §1.2)。
5. 対話操作は fxg が所有する PTY でのみ提供する。エージェント端末への入力は
   v1 では提供しない（プロトコルに無い）。

### 3.2 目標

- どのドライバでも「エージェントが実行したコマンド」を同じ見た目で追える。
- fxg が所有する端末は、表示・停止・（可能なら）入力まで一貫して操作できる。
- 停止できない場合は「できない」と UI で説明できる（無効なボタンを出さない）。

### 3.3 非目標

- エージェント内部プロセスの PID を探して kill すること。
- ACP v1 への独自拡張（`_meta` で入力 method を追加する等）。
- シェル統合（command blocks / cwd 追跡 / プロファイル）。
- 端末状態のノード間移行・DB 永続化（出力イベントを除く）。

## 4. 選択肢

### A. 現状維持（ACP v1 のクライアント端末のみ）

委任するエージェントだけ単体停止できる。実装コストはゼロだが、OpenCode
ネイティブや自前 bash のエージェントでは「ターン中断」しかできない。
ACP v2 で v1 端末が消えると、この経路自体が使えなくなる。

### B. fxg が MCP 端末サーバーを公開する（本命）

ACP v2 の RFD も「クライアント側の特殊機能は MCP サーバーとして公開できる」と
示唆している。fxg がノード内で MCP サーバーをホストし、端末操作をツールとして
提供する。

- ツール案: `terminal_create` / `terminal_send` / `terminal_read` /
  `terminal_kill` / `terminal_close`（作成時に `terminal_id` を返す）。
- fxg がプロセスを所有するため、表示・永続化・停止・入力・スナップショットが
  すべて既存の仕組みに乗る。ACP v1 / v2 / OpenCode / カスタムエージェントの
  どれでも同じ UX になる。
- 課題:
  - MCP サーバーの注入方法がエージェントごとに違う（OpenCode は MCP 設定、
    ACP エージェントは MCP サーバー指定）。設定 UI と保存先
    (`agent_settings`) が必要。
  - ツール名の衝突と、エージェント側の権限（permission）連携。
  - cwd の封じ込め（セッションの `local_path` 配下）と env の扱い。
  - 端末出力をモデルに返す際の上限（`outputByteLimit` 相当）。
- 既存の `terminal/*` 実装（`fxg-pty`、ライフサイクルイベント、kill）を
  そのまま流用できるのが利点。

### C. OpenCode の experimental persistent PTY に接続する

OpenCode は `/api/pty`（location スコープ）と、実験的な per-session
persistent PTY (`/api/experimental/session/{id}/terminal`,
`/api/experimental/persistent-pty/{ptyID}/connect`) を持つ。後者は
「そのセッションが最後に操作したターミナル」を read / takeover できる。

- 読み取り（スクリーンを行として取得）は比較的低リスクで、「エージェントが
  今何をしているか」を端末として見せる価値がある。
- 停止・入力は takeover と Ctrl-C に依存し、**experimental（互換保証なし）**。
  エージェントの bash が PTY 経由で動く構成かどうかもバージョン依存。
- fxg の統合ターミナルとは別の PTY なので、UI 上は「エージェント端末」として
  区別する必要がある。
- 導入するなら feature flag 付きの読み取り専用から始める。

### D. ACP v2 のエージェント所有 Terminal Output を受ける

v2 では端末はエージェントが所有し、クライアントは表示専用のストリームを受け取る
（Terminal Output RFD）。停止はエージェントの責務になる。

- v2 対応時に必須。表示と永続化（開始・差分・終了）は v1 の実装を流用できる。
- `kill_terminal` は v2 では「エージェントに停止を依頼する」操作に意味が変わる。
  そのためのツール／method が v2 に無ければ、UI は停止ボタンを出さない。

### E. OpenCode の `session/shell` と `/api/pty` を fxg から使う

- `POST /api/session/{id}/shell` は**クライアント発**のコマンド実行で、
  エージェントの bash ツールとは別物。停止 UX の解決にはならない。
- `/api/pty` で fxg が PTY を作ると、fxg 自身の統合ターミナルと二重になる。
- 採用するとしても「fxg からセッション cwd でコマンドを実行する」機能であり、
  本 proposal の主目的（エージェントのコマンド管理）とは別。

### 4.1 推奨

1. 当面は A（実装済み）で運用する。
2. 次に B（MCP 端末サーバー）を本命として設計する。
3. C は補助（experimental フラグ付き・読み取り専用から）。
4. D は ACP v2 対応時に受ける。
5. E は別機能として切り出す（必要になったら）。

## 5. プロトコル / データの扱い

型の正は `crates/fxg-protocol` と生成 bindings。ここでは意味だけを定める。

- MCP 端末ツールの結果は `TerminalOutput` と同じライフサイクル（開始・差分・
  終了）に正規化し、UI は端末カードとして描画する。MCP 固有の型を UI に
  漏らさない。
- 端末の同一性は fxg が発行する `terminal_id` に統一する（ACP の ID も MCP の
  ID も内部で同じレジストリに写す）。
- 「実行中かどうか」は `TerminalOutput.running` で表す（実装済み）。最終イベント
  だけが exit code を持つ。
- 停止可否はドライバではなく端末の所有権で決まる。UI は端末カードにのみ停止
  ボタンを出し、ツールカードには出さない。

## 6. 段階計画

### Phase 0（実装済み・2026-11-18）

- ACP v1 端末の開始/スナップショット/終了イベント。
- `SessionControlAction::KillTerminal` と `ActiveSessionHandle::kill_terminal`。
- 端末カードの停止ボタン、実行中カードを折りたたまない表示。
- セッション終了時のスナップショット待ちとレジストリ破棄。
- Windows の終了経路を Job Object 経由に修正（`portable-pty` 0.9 の
  `ChildKiller` は成功をエラーとして返すため）。

### Phase 1: MCP 端末サーバー（読み取り → 停止 → 入力）

- `terminal_create` / `terminal_read` / `terminal_kill` / `terminal_close` を
  ノード内 MCP サーバーとして公開する。出力は既存の `TerminalOutput` に正規化。
- エージェントへの注入: OpenCode は MCP 設定、ACP エージェントは起動時の
  MCP サーバー指定。設定は `agent_settings` に保存し、UI から有効化する。
- `terminal_send`（入力）は停止より後に。既存の `/api/v1/pty/ws` の
  入力経路を内部で再利用する。
- 受け入れ条件: OpenCode ネイティブのセッションで、MCP 経由で起動した
  長時間コマンドが端末カードに出て、UI から停止でき、リロード後も出力が残る。

### Phase 2: OpenCode persistent PTY（experimental・読み取り専用）

- feature flag 付きで `/api/experimental/session/{id}/terminal/read` を
  ポーリング／購読し、「エージェント端末」タブとして表示する。
- takeover と Ctrl-C は、experimental API の安定性とエージェントの実行形態を
  確認してから判断する。

### Phase 3: ACP v2 Terminal Output

- v2 に接続する際に、エージェント所有の表示専用ストリームを
  `TerminalOutput` に正規化する。停止ボタンは v2 の手段が決まるまで出さない。

## 7. リスク

- **MCP 注入の複雑さ**: エージェントごとに設定方法が異なり、未対応の
  エージェントでは機能しない。段階的に広げる。
- **出力量**: 差分イベントの合計が大きくなる。`outputByteLimit` と UI の
  上限（200k 文字）で抑えるが、長命なサーバーのログはターンごとに増える。
- **セキュリティ**: MCP 経由でも cwd はセッションの `local_path` に封じ込め、
  permission ポリシーと env を検証する。クライアント側 checks はエージェントの
  permission を置き換えない（ACP と同じ原則）。
- **experimental API の互換性**: OpenCode の persistent PTY は予告なく変わる。
  feature flag と失敗時のフォールバック（ツールカード表示）を必須にする。
- **v1/v2 の混在**: v2 エージェントでは `kill_terminal` が使えない。UI は
  「このエージェントでは単体停止できません」を出す。
- **端末の重複**: 統合ターミナル・ACP 端末・OpenCode PTY が並ぶと混乱する。
  UI 上は「自分の端末」と「エージェントの端末」を分け、相互リンクは
  Phase 1 以降に検討する。

## 8. 未決事項

- MCP サーバーをノード内蔵プロセスにするか、別プロセス（stdio）にするか。
  ノードの認証・監査・権限モデルとどう接続するか。
- MCP ツールの粒度（`terminal_*` を 1 つのツールにまとめるか、複数にするか）
  と、ACP の `terminal/*` との意味の対応。
- 端末カードと統合ターミナルの相互リンク（「Dock で開く」）を Phase 1 に入れるか。
- OpenCode の `session/shell` を「fxg からのコマンド実行」機能として切り出すか。
- ACP v2 の Terminal Output RFD が確定したときの `kill_terminal` の扱い。

## 9. 受け入れ条件（Phase 0）

- エージェントが `terminal/create` で起動したコマンドは、実行中からコマンド名が
  端末カードに出る。
- ターン終了後もカードと出力が残り、リロードで復元される。
- 端末カードの停止ボタンでそのコマンドだけが終了し、`exit` バッジに変わる。
- 停止してもターンやセッションは終了しない。
- 停止後のプロセスが残っていない（Unix / Windows とも）。
- 停止できない経路（エージェント自前実行・OpenCode ネイティブ）では停止
  ボタンを出さない。

## 10. 実装状況 (2026-11-18)

Phase 0 のみ実装済み。Phase 1〜3 は [`docs/backlog.md`](../backlog.md) §7 に残す。

- 実装: `TerminalOutput.running`、開始/スナップショット/終了イベント、
  `SessionStatus::is_busy`、`KillTerminal`、`kill_terminal`、
  `snapshot_terminals`、端末カードの停止ボタン、Windows の Job Object 終了経路。
- 変更履歴:
  [`docs/changelog/2026-11/2026-11-18-agent-terminal-cards.md`](../changelog/2026-11/2026-11-18-agent-terminal-cards.md)
