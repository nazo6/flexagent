# 02. データベーススキーマ設計（`node.db` / `hub.db` 単一スキーマ）

FlexAgentは外部DBミドルウェアを必要としない **SQLite (`sqlx`)**
を採用し、WALモード (`PRAGMA journal_mode = WAL;`) で動作します。
E2E暗号化を行わないため、ハブ側で会話履歴の構造化クエリおよび
**FTS5（全文検索）** がそのまま利用可能です。

`node.db`（各ノード: `~/.flexagent/node.db`）と `hub.db`（ハブ:
`~/.flexagent/hub.db`）は、**同一のDDL・同一のテーブル名・単一のマイグレーションセット**
（`crates/fxg-db/migrations/`）から生成される「1つのスキーマ」を共有します。
両者の違いは **「行のスコープ」と「実行ロール」だけ**
であり、スキーマ対称性は「規約」ではなく 「単一定義」によって保証されます。

---

## 0. 単一スキーマ原則

### 0.1 3つの原則

1. **スキーマは1本**: 役割専用のテーブル（`push_subscriptions` 等）やカラム
   （`synced_up_to_node_seq` 等）も両DBに作成し、使わない側では 空 /
   未使用のまま放置する。DDL・クエリ・マイグレーションを1本に保つことを優先する
   （空テーブル・未使用カラムの実害はない）。
2. **同期はイベントログ1本**:
   セッションのメタデータ（作成・タイトル・ステータス・モード）も
   `session_events` 上のイベントとして記録する。ハブの `sessions` /
   `permission_requests` はイベントから適用される**投影（projection）** であり、
   別系統のメタデータ同期は存在しない。
3. **書き込み権威は1つ**: セッション状態を書き換えられるのは**実行ノードのみ**。
   ハブは (a) クライアントコマンドの転送 (b)
   受信イベントの追記と投影適用
   だけを行い、クライアント要求に応じてセッション状態を直接書き換えない。

### 0.2 ロール差分（ノード / ハブ）

| 項目                                 | ノード (`node.db`)                                   | ハブ (`hub.db`)                                                    |
| :----------------------------------- | :--------------------------------------------------- | :----------------------------------------------------------------- |
| `nodes`                              | 自分自身の1行のみ（FK参照・ローカルUI用）            | 全ノード（`NodeHello` で更新。`token_hash` はハブのみ使用）        |
| `projects` / `project_node_bindings` | 自ノードが関与する分のみ                             | 全ノード分（ノード報告の集約。ハブが権威）                         |
| `sessions`                           | **権威**。自分のセッションのみ。直接書き込む         | **投影**。全ノード分。イベント適用でのみ更新                       |
| `session_events`                     | 自分のセッションのイベント。水位より後が Outbox 対象 | 全イベントの冪等追記。`cursor` はハブ配信用カーソル                |
| `session_events_fts`                 | ローカル直結UIの検索用                               | 全セッション横断検索用                                             |
| `permission_requests`                | 自分のセッション分の投影                             | 全会話分の投影（グローバル承認 Inbox）                             |
| `push_subscriptions`                 | 空（未使用）                                         | PWA 購読管理（VAPID、`client_id` で端末に紐付け）                  |
| `audit_logs`                         | ローカル直結操作（CLI / `localhost:7860`）の記録     | ハブ経由操作の記録（同期対象外。各ストアが自分の観測事実のみ記録） |
| `incidents`                          | ノード自身のエラー境界で観測した障害                 | ハブ自身のエラー境界で観測した障害（同期対象外）                   |
| `sessions.synced_up_to_node_seq`     | 使用（ハブが ACK した水位）                          | 未使用（常に 0）                                                   |
| `sessions.last_node_seq`             | 永続化済み最新の `node_seq`（次はこの値 + 1 を採番） | 投影に適用済みの最大 `node_seq`                                    |

クライアントの可視 / フォーカス状態（Presence）は通知先の一時判定にのみ使い、
DB には保存しない。ハブ再起動後は空になり、通知のフォールバックが適用される。

### 0.3 イベント適用と投影

ハブは `EventBatchPush` を受信すると、**同一トランザクション**で
イベントを `INSERT OR IGNORE`（`event_id` / `(session_id, node_seq)` の UNIQUE
制約で 冪等）し、続けて投影を更新します。投影の更新規則は以下で固定です：

```text
SessionCreated        → sessions 行の生成（project_id / node_id / local_path /
                        git_branch / is_worktree / agent_id /
                        parent_session_id / title / created_at）
                        ※ node_id は SessionCreated payload が保持する
                          (イベントログのみから投影を完全再構築可能にするため)
SessionTitleChanged   → sessions.title
SessionAgentBound     → sessions.agent_session_id
StatusChanged         → sessions.status
CapabilitiesUpdated   → sessions.current_mode / available_modes_json /
                        available_commands_json / config_options_json /
                        fork_scope
UsageUpdated          → sessions.usage_json (最新の used / size / cost。
                        累積ではなく最新の `usage_update` を上書き)
すべてのイベント      → sessions.last_node_seq = 適用済み最大 node_seq
                        sessions.updated_at = 最新イベントの created_at
PermissionRequest     → permission_requests 行を upsert (status = 'pending')
PermissionResolved    → permission_requests.status / resolved_by / resolved_at
ElicitationRequest    → elicitation_requests 行を upsert (status = 'pending')
ElicitationResolved   → elicitation_requests.status / content_json / resolved_by /
                        resolved_at
SessionArchived       → sessions.archived_at (archived = false で NULL に戻す)
SessionPermissionPolicyChanged → sessions.permission_policy
SessionDeleted        → sessions.deleted_at = tombstone イベントの created_at
                        + 本文イベントのパージ
                        (本イベント (tombstone) と sessions 行は残す)
```

- 投影はイベントログから**完全に再構築可能**です。`fxg-db` は再構築関数
  （イベント全量からの再生）を提供し、テストで「イベント適用後の状態 ==
  全量再構築後の状態」を 検証します。
- 未知のセッションのイベントは、バッチ内の `SessionCreated`（`node_seq = 1`）を
  先に適用することで FK 制約を満たします（イベントは `node_seq`
  昇順で送信される）。
- **削除済みセッションへの追記は破棄**します（tombstone 適用後の Resync
  再送や停止処理との競合で、パージ済みの会話内容が復活するのを防ぐ）。
  未知セッションへの `SessionDeleted`（Resync 由来）は FK 制約違反にせず
  no-op とします。

### 0.4 同期の水位（ACK）とカーソル

- **水位（Watermark）**:
  「送信済み」は行単位フラグではなく、セッション単位の水位
  `sessions.synced_up_to_node_seq` で表現します。`node_seq`
  は実行ノードだけが採番する
  欠番のない連番のため、水位による表現が成立します。Outbox 抽出は
  `node_seq > synced_up_to_node_seq` の単純な述語になり、ACK 時の更新も O(1)
  です。
- **カーソル (`session_events.cursor`)**: そのDB（`node.db` / `hub.db`）が
  イベントを取り込んだ順の AUTOINCREMENT 連番です。クライアント（CLI / Web
  UI）の 差分再開カーソルとして使用します。**接続先ストア以外の cursor
  は意味を持ちません** （ハブ接続時はハブの cursor、ローカル直結時はノードの
  cursor を使用）。
  - 重複イベントは IGNORE されるため cursor には欠番が生じ得ますが、再開は
    `cursor > 最後に受信した cursor` で行うため欠番は問題になりません。

---

## 1. テーブル構成と責務

DDL・カラム・インデックスの正は `crates/fxg-db/migrations/` です。ここでは
テーブル間の責務と、ノード / ハブのロール差分だけを扱います。

| テーブル                                                                                      | 責務                                                                                                                  |
| :-------------------------------------------------------------------------------------------- | :-------------------------------------------------------------------------------------------------------------------- |
| `nodes`                                                                                       | ノード登録簿。ハブは全ノード、ノードは自分自身の1行。`token_hash` はハブのみ使用。                                    |
| `projects` / `project_node_bindings`                                                          | 論理プロジェクトと、ノード上のローカルパス / Worktree の紐付け。                                                      |
| `sessions`                                                                                    | セッション状態。ノードが権威、ハブはイベントからの投影。承認ポリシー・使用量・アーカイブ / 削除状態もここに投影する。 |
| `session_events`                                                                              | Append-only のイベントログ。`cursor` はストアへの取り込み順、`node_seq` はセッション内の論理順。                      |
| `session_events_fts`                                                                          | `session_events` の FTS5 (trigram) 検索インデックス。                                                                 |
| `permission_requests` / `elicitation_requests`                                                | 未解決の承認 / 質問を引くための投影テーブル。                                                                         |
| `push_subscriptions`                                                                          | ハブの Web Push (VAPID) 購読。                                                                                        |
| `audit_logs`                                                                                  | 各ストアが自分の観測した操作を記録する監査ログ。同期対象外。                                                          |
| `incidents`                                                                                   | 各ストアのエラー境界が観測した障害のリングバッファ。同期対象外。                                                      |
| `node_settings` / `agent_settings` / `agent_aliases` / `custom_agents` / `project_path_links` | ノード所有の実行時設定。                                                                                              |
| `hub_settings` / `provisioners`                                                               | ハブ所有の実行時設定。                                                                                                |
| `secrets`                                                                                     | 秘密のメタデータ。値は `~/.flexagent/secrets/{node,hub}/<id>` に置く。                                                |
| `node_pairing`                                                                                | ノードのハブ接続先。トークンは `~/.flexagent/node_token` に置く。                                                     |

### 1.1 重要な不変条件

- `session_events` は `event_id` と `(session_id, node_seq)` で一意。
- `sessions.last_node_seq` はノードでは永続化済み最新、ハブでは投影に適用済み最大。
- `sessions.synced_up_to_node_seq` はノード専用の ACK 水位。ハブでは未使用。
- `sessions.deleted_at` は tombstone。行は残し、本文イベントと承認 / 質問をパージする。
- `sessions.archived_at` は可逆な可視性フラグ。
- `sessions.permission_policy` は `ask` / `auto_allow_all`。
- `incidents` は同じ `(code, source, session_id, node_id)` の未解決障害を10分以内なら集約し、14日・最大1000行で保持する。

### 1.2 設定の所有者

実行時に変更する設定は所有者の DB にのみ保存し、ノード ⇔ ハブ間で同期・マージしない。

| 設定                                                                                                           | 所有者 / 保存場所                               |
| :------------------------------------------------------------------------------------------------------------- | :---------------------------------------------- |
| `node_settings` / `node_pairing` / `agent_settings` / `agent_aliases` / `custom_agents` / `project_path_links` | `node.db`                                       |
| `hub_settings` / `provisioners`                                                                                | `hub.db`                                        |
| 秘密のメタデータ                                                                                               | `node.db` / `hub.db` の `secrets`               |
| 秘密の値                                                                                                       | `~/.flexagent/secrets/{node,hub}/<id>` (`0600`) |

単一行テーブルは `revision` による楽観的ロックを行い、競合は
`REVISION_MISMATCH` として返します。マイグレーションの意図は
[`changelog/2026-11-10-architecture-simplification.md`](changelog/2026-11-10-architecture-simplification.md)
を参照してください。

## 2. 正データと派生カラム

イベントログを唯一の正（Single Source of
Truth）とするため、テーブル上の以下のカラムは
すべて**派生データ**として扱います。派生カラムはクエリ性能のためにのみ存在し、
常にイベントログから再構築できることが要件です（再構築関数 + テストで保証）。

| 派生カラム / テーブル                          | 生成源                                                                     |
| :--------------------------------------------- | :------------------------------------------------------------------------- |
| `sessions.*`（`synced_up_to_node_seq` を除く） | `session_events` の適用（§0.3 の投影規則）                                 |
| `permission_requests`                          | `PermissionRequest` / `PermissionResolved` イベント                        |
| `elicitation_requests`                         | `ElicitationRequest` / `ElicitationResolved` イベント                      |
| `session_events.searchable_text`               | `payload_json` からのテキスト抽出（`TerminalOutput` 等のバイナリ系は除外） |

- `searchable_text` の生成責務はイベントを永続化する側（ノード /
  ハブ双方の受信ハンドラ）にあります。
- **削除は行を消さない (tombstone)**: `SessionDeleted` の適用時も `sessions`
  行は削除せず `deleted_at` を立て、本文イベントと `permission_requests` /
  `elicitation_requests`
  を物理削除します。行を残すのは Outbox / Resync
  で削除を伝播し、Fork 元参照 (`parent_session_id`)
  の FK 整合を保つためです（保存量は数百バイト）。削除済みセッションは
  一覧・取得の双方から除外され (`deleted_at IS NULL`)、イベント追記は
  破棄されます（再送 tombstone 自体は冪等）。
- **アーカイブは可逆な可視性フラグ**: `archived_at` が非 NULL
  のセッションは既定の一覧から除外され、`include_archived`
  指定時のみ返却されます。イベントログは保持され、`SessionArchived {
  archived: false }` で復元できます。
- **一時VMの仮セッション行**: 一時VMプロビジョニング中（`NodeHello` 前）は、
  ハブが `sessions` 行を `status = 'provisioning'`
  で先行挿入します（`upsert_provisional_session`。イベントではなく投影行）。
  ノードの `SessionCreated` が upsert で派生カラムを上書きし、以降は通常の
  イベント投影に合流します（全量再構築時には失われ得るベストエフォートの表示用状態）。

---

## 3. クエリ・同期・運用のポイント

- **共通クレート `fxg-db`**:
  単一スキーマ・単一マイグレーションセット・単一のクエリ層を持ち、
  実行ロール（`Node` / `Hub`）パラメータで挙動（Outbox 送信 / 中継 / Push
  等）だけを分岐させます。 ローカルWeb UI（`http://localhost:7860`）が `node.db`
  を読む際も、ハブWeb UIが `hub.db`
  を読む際も、同じJSONレスポンス型（`SessionDetail`,
  `SessionEvent`）を返却できます。
- **ノードの自己登録**: ノードは起動時に `nodes` テーブルへ自分自身の行を upsert
  し、 プロジェクト解決結果を `projects` / `project_node_bindings` へ upsert
  します （ハブへの `NodeHello` 報告と同一の情報源）。
- **水位ベース Outbox**: Outbox Sync Worker は
  `SELECT ... FROM session_events e JOIN sessions s USING (session_id)
  WHERE e.node_seq > s.synced_up_to_node_seq ORDER BY e.node_seq`
  で未送信イベントを抽出し、 `EventBatchAck` 受信時に
  `sessions.synced_up_to_node_seq` を更新します。
- **ハンドシェイク再同期**: `NodeHello`（`SessionSyncState` 一覧）とハブの
  `last_node_seq` を比較し、 欠落・遅延のあるセッションは
  `ResyncRequest { sessions: [{ session_id, from_node_seq }] }` で
  再送を要求します（ハブDBを再構築した場合の自動復元）。
- **日本語・ソースコード検索に強い `trigram` トークナイザ**: SQLite FTS5の
  `tokenize='trigram'`
  を使うことで、形態素解析器なしで日本語の会話（「認証エラー」「データベース」）も関数名・識別子（`AgentDriver`）も高速に全文検索できます。
  - 制約: trigram は 3 文字未満の検索語ではヒットしないため、検索 UI では 3
    文字以上を要求するか、LIKE 検索へフォールバックします。
