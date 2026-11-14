# サブエージェント表示（opencode 拡張 + ACP subagents RFD）

- **日付**: 2026-11-12
- **状態**: 実装済み（擬似 ACP エージェントで検証）
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `fxg-node`, `fxg-cli`, `ui`,
  `docs`
- **対象スクリプト**: `mise run fmt`, `mise run types`,
  `mise run lint:errors`, `cargo test -p fxg-protocol -p fxg-acp -p fxg-node`,
  `pnpm --dir ui run check`, `pnpm --dir ui run test`

> 型・enum・REST の正は `crates/fxg-protocol/` と生成済み bindings、ACP の
> イベント処理は `crates/fxg-acp/` を参照。本文は設計意図と不変条件だけを
> 説明する。全体のイベントモデルは
> [`docs/03-protocol-and-api.md`](../../03-protocol-and-api.md)、ドライバ構成は
> [`docs/04-agent-drivers-and-windows.md`](../../04-agent-drivers-and-windows.md)
> を参照。

---

## 1. 目的

サブエージェント（opencode の `task`、Claude/Codex の Agent/Task など）の
会話・思考・ツール呼び出し・承認を、親セッションのタイムラインから
区別できる形で表示する。子は独立した fxg セッションにはせず、親の
イベントログにタグ付きで混在させる。

対象は次の 2 系統。どちらも同じ正規化イベントへ変換する。

- **opencode の vendor 拡張**: `opencode/session/child_update` 通知と
  `_meta.opencode/child-session`。
- **ACP 標準の subagents RFD**: `clientCapabilities.subagents` を広告した
  場合の `subagent_update`、子セッション宛の `session/update`、
  `session_message` / `session_message_chunk`（Claude / Codex など）。

agy の内部 teamwork は ACP に子セッションを公開しないため対象外とする。
子へのプロンプト送信（子を対話セッションとして操作する）も対象外。

## 2. 不変条件

- 子は `sessions` 行を持たず、`session/list` に現れない。親を
  アーカイブ・削除すると子のイベントも同じログとして扱う。
- `SubagentUpdated` が子の告知・状態更新の唯一の入口であり、子の他の
  イベントより先に永続化される。
- 子由来のイベントは `subagent_id` を持つ。親の `sessions` 投影
  （`status` / `usage_json` / `current_mode` / `title`）は
  `subagent_id` のないイベントだけで更新する。
- 子イベントは親と同じストリーミング結合・順序・再接続・ハブ同期の経路を
  使う。別のキューや別 DB を作らない。
- 承認の request ID は親セッションのまま。子の承認も既存の Inbox /
  Web Push / CLI の経路で解決できる。
- vendor 固有の通知解析は extension に閉じ、`AcpDriver` にエージェント名の
  分岐を足さない。

## 3. 設計

### 3.1 ACP capability の広告

ACP の機能は、接続時にエージェントへ広告する capability で制御する。

- **Cargo**: `agent-client-protocol` の必要な unstable feature
  （`unstable_session_fork` / `unstable_subagents`）は依存で常に有効化する。
  ビルド時に分離したい場合だけ `fxg-acp` の Cargo feature を追加する。
- **Runtime**: `AcpCapabilitySet`（未知の名前を保持する open enum の集合）を
  エージェント定義の `AcpExtensionOptions` として受け取り、`initialize` の
  client capabilities に反映する。既定は `subagents` を ON。未知の
  capability は警告して無視する。プロセス全体の環境変数では上書きしない
  （capability は接続ごと、つまりエージェントごとに決まるもの）。
- 拡張は `AcpExtensionOptions` で `clientCapabilities._meta`、受信する独自
  通知の method、追加の capability を提供できる。
- ACP 標準の `subagents` は unstable RFD のため、既定 ON としつつ
  エージェント定義ごとに外せる形にしておく。

### 3.2 通知ルーティング

`agent-client-protocol` のセッションルーターは root セッション ID 宛の通知
だけを `serve_session` に流す。子宛の通知はそのままでは捨てられるため、
`fxg-acp` が次の境界を持つ。

- root 宛の `session/update` は従来どおり `ActiveSession` が受ける。
- 子宛の `session/update` は、子を告知した時点で子ごとの dynamic handler を
  登録して受ける。子の ID 以外は `Handled::No` として root の経路を壊さない。
- vendor の独自通知は、extension が宣言した method だけを method handler で
  受ける。builder の catch-all は root の `session/update` を奪うため使わない。
- 受けた子イベントは親と同じ accumulation に合流させ、`subagent_id` を付けて
  正規化する。
- `SessionContexts` に子の `cwd` を登録し、子からの fs / terminal /
  permission 要求を子のコンテキストで解決する。子を告知しないエージェントの
  ために「登録済みコンテキストが 1 つだけなら親へフォールバック」も残す。
- 子の状態は `SubagentUpdated` で表し、親の `StatusChanged` を発行しない。

### 3.3 opencode 拡張

`OpenCodeExtension` が vendor capability と通知 method を宣言し、通知の
解析器を提供する。解析結果は子 ID・親 ID・depth・title・状態・内側の
`SessionUpdate` を持つ汎用の子更新へ変換し、コアの正規化に渡す。

- 子のツール ID は vendor 側で `childId:toolCallId` に接頭辞化される。
  一意性のためそのまま保持する。
- 子の承認は root セッション ID + `_meta` で届く。`SessionExtension` が
  `_meta` を読み、`subagent_id` を付与する。
- capability を有効にすると、子の更新は vendor 通知へ切り替わる。ハンドラの
  実装と capability の広告は同じ変更で行う。解析に失敗した場合は警告し、
  従来のフラット表示にフォールバックできるようにする。

### 3.4 ACP 標準 RFD

`subagent_update` を `SubagentUpdated` へ、子セッション宛の通常の
`session/update` を `subagent_id` 付きイベントへ変換する。`state` は
`running` / `requires_action` / `completed` / `failed` / `cancelled` に
正規化する。`cwd` / `additionalDirectories` は子のコンテキストに反映する。

`session_message` / `session_message_chunk` は親子間メッセージとして
`SubagentMessage` に変換し、送信元・宛先で方向を保持する。完成イベントだけを
永続化し、ストリーミング中は delta として配信する。

子のキャンセルは、子が `capabilities.cancel` を広告している場合だけ
子セッション ID 宛の `session/cancel` として送る。

### 3.5 UI

- **インライン**: 最初の子イベントの位置にサブエージェントブロックを置く。
  親の `task` ツールカードへの厳密なネストはしない（vendor が起動ツール
  コール ID を返さないため）。
- **一覧**: セッションヘッダーの「N subagents」から、タイトル・状態・depth・
  使用トークンを一覧表示する。
- **詳細**: デスクトップは右サイドシート、モバイルは全画面ドロワー。
  子のメッセージ・思考・ツール・plan・usage・承認をフル表示する。
  `?subagent=<id>` でディープリンクし、ネストはパンくずで辿る。
- インラインと詳細は同じイベントレンダラを使い、`subagent_id` で絞り込む。
- 親のコンテキストメーターと plan は親のイベントだけを使う。子の usage は
  子ブロック / 詳細に表示する。
- ライブ更新は delta の `subagent_id` で子ブロックへ配置する。リロード後は
  永続イベントから再構築する。

## 4. 実装フェーズ

1. **P0 プロトコル**: `SubagentUpdated` / `SubagentMessage` /
   `subagent_id` / delta のフィールドを追加し、型生成と docs を同期する。
2. **P1 ACP seam**: `AcpExtensionOptions` / `AcpCapabilitySet` /
   notification parser / 子ごとの dynamic handler / `SessionContexts` の
   子コンテキストを追加する。
3. **P2 ACP RFD**: `unstable_subagents` を有効化し、標準イベントのマッピング、
   client capability、子キャンセル、設定の kill switch を追加する。
4. **P3 opencode**: vendor capability、通知 parser、承認の子ラベル、
   フラット表示へのフォールバックを追加する。
5. **P4 UI**: インラインブロック、一覧、詳細シート、ディープリンク、
   ストリーミング配置、usage / plan の分離を追加する。
6. **P5 検証・docs**: 擬似 ACP エージェント、実機 opencode の `task`、
   Claude/Codex（可能な範囲）、changelog と設計ドキュメントの更新。

## 5. 検証

- 擬似 ACP エージェント（`crates/fxg-acp/tests/fixtures/mock_agent.rs`）に
  vendor 通知モードと ACP RFD モードを追加し、子の text / tool / permission /
  session_message を検証する。
- warm pool で 2 セッション連続実行し、子ごとの dynamic handler と
  `SessionContexts` がリーク・混線しないことを確認する。
- reducer のユニットテストで、ツリー構築、`subagent_id` フィルタ、
  ストリーミング、usage / plan の分離を検証する。
- 実機で opencode の `task` を実行し、子の会話と承認の出所が表示されることを
  確認する。Claude/Codex は認証が必要なため手動確認とする。

## 6. 非目標

- サブエージェントを独立した fxg セッションとして一覧・操作すること。
- 子へのプロンプト送信・キューイング。
- agy など、ACP に子セッションを公開しないエージェントの内部 teamwork 表示。
- 子の会話を親とは別の DB / 検索インデックスに分離すること。

## 7. 実装ログ

- **プロトコル**: `SubagentUpdated` / `SubagentMessage` と `subagent_id` を
  イベント・delta に追加した。子 usage は親の `sessions.usage_json` 投影に
  反映しない。
- **ACP seam**: `AcpExtensionOptions` / `AcpCapabilitySet` / notification parser
  を追加し、子宛 `session/update` は子ごとの dynamic handler、vendor 通知は
  method handler で受ける。子は `SessionContexts` に `cwd` 付きで登録する。
- **opencode**: `opencode/session/child_update` を解析し、子の text / tool /
  承認を正規化する。`_meta` の child ID で承認をタグ付けする。vendor は
  更新のたびに同じ status を再送するため、内容が変わらない
  `SubagentUpdated` は送らない。
- **ACP RFD**: `clientCapabilities.subagents` を既定で広告し、
  `subagent_update` / 子 `session/update` / `session_message` を処理する。
  広告するかどうかはエージェント定義の `AcpExtensionOptions` が持つ。
- **UI**: 最初の子イベント位置にサブエージェントブロックを表示し、
  ヘッダーの「Open」から詳細シートを開ける。承認カードには子のタイトルを
  表示する。
- **per-agent 設定**: `subagents` ON/OFF は `CustomAgentConfig.capabilities`
  （`node.db` の `custom_agents.capabilities_json`）に保存し、custom agent の
  編集フォームで変更する。ビルトインは既定値のまま読み取り専用で、一覧の
  バッジに有効な capability を表示する。opencode は vendor `_meta` で子を
  公開するため、このトグルは標準 capability と vendor 通知の両方を止める
  （`acp_options` が実効 capability を受け取る）。`AgentSummary` は実効
  capability と拡張 ID を返す。
- **ビルトインの拡張 ID**: `with_extension("opencode", ...)` のように
  ビルトイン provider が拡張 ID を明示し、カタログはその ID で登録する。
  一覧・設定ダイアログは拡張 ID を表示する。
- **検証**: `crates/fxg-acp/tests/subagents.rs`（擬似 ACP エージェント）で
  RFD と opencode の子イベントを確認。`cargo test --workspace`,
  `cargo clippy`, `pnpm --dir ui run check`, `pnpm --dir ui run test` が通る。
  実 opencode の `task` は手動確認項目として残る。

## 8. リスク

- ACP の subagents RFD は unstable。型・意味が変わる可能性があるため、
  feature と実行時設定で隔離する。
- root と子のストリームは別経路のため、両者間の完全な全順序は保証しない。
  子ストリーム内の順序と、`node_seq` による表示順は保証する。
- vendor の接頭辞・通知形式は実装詳細であり、バージョン差で変わり得る。
  解析失敗時は警告し、従来表示へフォールバックする。
- バックグラウンド子が親ターン終了後も動く場合、親の `StatusChanged` と
  子の状態は独立する。ヘッダーの「subagents running」表示は別途扱う。
