# 通知系（Web Push / アクティブクライアント / 通知設定）実装計画

- **日付**: 2026-11-13
- **状態**: 実装中（Phase 2 完了）
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-hub`, `ui`, `docs`
- **対象スクリプト / 設定**: `mise run fmt` / `mise run lint` /
  `mise run lint:errors` / `mise run types` / `mise run sqlx:prepare` /
  `mise run test` / `pnpm --dir ui run check`
- **関連ドキュメント**:
  - [`docs/01-architecture-and-sync.md`](../01-architecture-and-sync.md) §3
  - [`docs/02-database-schema.md`](../02-database-schema.md)
  - [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md) §3.1, §3.2, §4
  - [`docs/05-cli-and-pwa-ui.md`](../05-cli-and-pwa-ui.md) §3.3
  - [`2026-11-09-daily-driver-gaps.md`](2026-11-09-daily-driver-gaps.md) §6.2 (G8)
  - [`2026-10-02-acp-enhancement-plan.md`](2026-10-02-acp-enhancement-plan.md)
  - [`2026-11-08-english-code-and-i18n-policy.md`](2026-11-08-english-code-and-i18n-policy.md)

---

## 0. 目的とスコープ

通知を「承認リクエストだけが、全端末に届く」状態から、次の状態へ引き上げる。

1. **通知種別を揃える**: 承認・質問 (elicitation)・ターン完了。
2. **送信先を選べる**: いま見ている端末だけ / 直近使った端末 / 全端末。
3. **通知設定を持つ**: 種別ごとの on/off、宛先ポリシー、フォールバック。
4. **配信と購読を運用できる**: タイムアウト、失効購読の掃除、購読一覧・削除、
   鍵不整合の検知。
5. **通知タップの UX を直す**: 既存ウィンドウを正しいセッションへ遷移させる。

### スコープ外

- APNs / FCM などネイティブ Push、メール・Slack 連携。
- マルチユーザー / 権限分離（現状は単一トークン）。
- ローカルノード直結時の Push（ハブ専用のまま。§6.4）。
- iOS 16.4 未満、Push API 非対応ブラウザ。

---

## 1. 現状と課題（コード実測）

| 領域             | 現状                                                                                                     | 課題                                                                                                                                        |
| :--------------- | :------------------------------------------------------------------------------------------------------- | :------------------------------------------------------------------------------------------------------------------------------------------ |
| 承認 Push        | `PermissionRequest` を検出し全購読へ送信。SW の Approve / Reject は API を直接叩く                       | 動作する。ただし宛先制御・解決済み通知の後始末が無い                                                                                        |
| 質問 Push        | `notify_elicitations` と payload は存在する                                                              | `hub.rs` の通知対象に `ElicitationRequest` が積まれず**未発火**（実質デッドコード）                                                         |
| ターン完了 Push  | `notify_turn_finished` と payload は存在する                                                             | `running_sessions` が `apply_event_batch` 呼び出しローカル。ライブ同期は 1 イベント / バッチのため、`Idle` 時には空になり**ほぼ発火しない** |
| 端末識別         | `push_subscriptions` は `endpoint` / `p256dh` / `auth` / `device_name` のみ。`device_name` は User-Agent | 端末ごとの宛先制御・購読管理ができない                                                                                                      |
| クライアント識別 | Client WS は接続 ID を持たず、ハブはブロードキャストのみ。`ClientInfo` は IP / UA / auth 種別            | 「どのクライアントがアクティブか」を知る手段が無い                                                                                          |
| フォーカス情報   | UI は `Subscribe { focused_session_id }` を送信済み                                                      | ハブ側で未使用。通知の抑止に使える                                                                                                          |
| 通知設定         | `HubSettings` は `vapid_subject` のみ                                                                    | 種別トグル・宛先ポリシーが無い                                                                                                              |
| 購読管理         | `POST /api/v1/push/subscribe` のみ                                                                       | 一覧・削除 API が無い。端末側 `unsubscribe` 後も行が残る                                                                                    |
| 配信             | 全購読へ直列送信、`reqwest::Client` はタイムアウト無し。404/410 のみ削除                                 | 1 台の死んだ endpoint が後続を待たせる。403（VAPID 鍵不一致）は永久に失敗し続ける                                                           |
| SW / 通知 UX     | `notificationclick` は既存ウィンドウを `focus()` するだけ                                                | タップしてもセッションへ遷移しない。全通知が `requireInteraction: true`。解決済み通知が残る                                                 |
| ローカルノード   | `push_subscribe` は `INVALID_STATE`、`vapid_public_key` は `null`                                        | 通知はハブ接続時のみ。UI は導線を隠している（仕様として明記する）                                                                           |
| docs             | `docs/05` §3.3 は質問通知が動く前提で記述                                                                | 実装と乖離。§6.5 で更新する                                                                                                                 |

---

## 2. 設計方針

### 2.1 用語

| 用語         | 定義                                                                                  |
| :----------- | :------------------------------------------------------------------------------------ |
| Client       | ブラウザプロファイル / インストール済み PWA。`client_id` で識別する                   |
| Tab          | 1 本の Client WS 接続。`tab_id` で識別する                                            |
| Active       | `visible && focused` かつ、サーバー受信時刻が TTL 内の Tab が 1 本以上ある Client     |
| Subscription | Web Push の endpoint。1 Client が複数持つことは通常無いが、PWA とブラウザで分かれ得る |

**サーバーはクライアント申告の時刻を信用しない。** presence の鮮度はハブの受信時刻で判定する
（端末の時計ずれ・改ざんの影響を排除）。

### 2.2 宛先ポリシー

`hub_settings` に保存するハブ全体の設定とする（通知はハブ専用のため）。

| ポリシー           | 送信先                                                       |
| :----------------- | :----------------------------------------------------------- |
| `all`              | 全購読（従来動作）                                           |
| `active_only`      | Active な Client の購読のみ                                  |
| `active_or_recent` | Active な Client + `recent_secs` 以内に Active だった Client |
| `most_recent`      | 最後に Active だった 1 Client のみ                           |

- `active_only` / `active_or_recent` で対象が 0 件のときは `fallback` に従う
  （`all` = 全購読 / `none` = 送らない）。
- **複数の Active Client** がある場合、`active_only` / `active_or_recent` は
  すべてに送る。`most_recent` だけが 1 台に絞る。
- **表示中セッションの抑止**: `focused_session_id` が通知対象セッションと一致する
  Tab には送らない（既定 on）。同じセッションを開いているのに通知で邪魔をしない。
- 既定値は `active_only` + `fallback = all`。アクティブ不在時に通知を落とさない。

### 2.3 Presence の収集

- `client_id`: `localStorage` に UUID を生成・永続化（ブラウザプロファイル単位）。
- `tab_id`: タブごとに `crypto.randomUUID()`（`sessionStorage` は使わない）。
- Client WS に `presence` メッセージを追加する。`visible` / `focused` /
  `focused_session_id` を送る。
- 送信タイミング:
  - `Subscribe` 直後（接続直後の初期状態）
  - `visibilitychange` / `focus` / `blur` / `pageshow` / `pagehide`
  - 30 秒ハートビート（hidden 中は 60 秒に延長してよい）
- ハブ側は in-memory の `PresenceRegistry` に保持する。**DB には保存しない**
  （通知先の一時状態であり、再起動後は fallback で足りる）。
- WS 切断時に該当 Tab を削除する。TTL は 90 秒。
- 複数タブ: 1 タブでも `visible && focused` ならその Client は Active。
- バックグラウンド / 画面ロック: `visibilitychange` が来ない環境があるため、
  ハートビート途絶を TTL で拾う。厳密な「画面を見ている」判定は保証しない（§6.3）。

### 2.4 購読と端末の紐付け

- `PushSubscribeRequest` に `client_id` を追加する（必須）。
- `push_subscriptions.client_id` を追加する（migration）。既存行は `NULL`。
- 既存購読の移行: UI 起動時に購読済みなら `/push/subscribe` を再送し、
  `client_id` を埋める（endpoint の upsert で冪等）。
- `client_id` が無い購読の扱い: `all` では送る。`active_*` / `most_recent` では
  Active 判定できないため、`fallback = all` のときのみ送る。UI の再登録で解消する。

### 2.5 通知の生成とルーティング

- イベント適用後に対象イベントを抽出して payload を作る（既存の
  `push.rs` の関数を拡張）。
  - `PermissionRequest` → 承認通知（既存）
  - `ElicitationRequest` → 質問通知（**新規に配線**）
  - `StatusChanged(Running → Idle | Error)` → 完了通知（**判定を修正**）
- ターン完了の判定はハブ状態で行う。`HubState` に「Running を観測したセッション」
  の集合を持ち、起動時に DB の `Running` セッションから seed する。
  `Running` 適用で追加、`Idle` / `Error` 適用で「存在したときだけ」通知して削除。
  これにより 1 イベント / バッチでも正しく動き、ハブ再起動をまたげる。
- resync バッチでは従来どおり通知しない（履歴再送で端末を起こさない）。
- 通知種別の設定（`notify_permissions` / `notify_questions` /
  `notify_turn_finished`）で生成を抑止する。
- 宛先選択は純粋関数に切り出し、テスト可能にする:
  `select_target_subscriptions(subscriptions, presence, settings, now, event)`
- tag は既存方針を維持する（承認は request 単位、質問は elicitation 単位、
  完了は session 単位）。

### 2.6 配信の堅牢性

- `reqwest::Client` に connect / total timeout を設定する（例: 10 秒）。
- 購読への送信は上限付き並列（例: 8 並列）にする。1 台の遅延が全体を止めない。
- 404 / 410 は購読を削除（既存）。401 / 403 は削除せず、警告ログと
  診断カウンタに記録する。VAPID 鍵の再生成が疑われる旨をログに含める。
- endpoint 全体をログに出さない（URL に購読の秘密が含まれるため、
  短縮またはハッシュにする）。
- 失敗は `DiagnosticsCounters::PushFailures` に集計（既存）。必要なら
  「最終成功時刻 / 連続失敗回数」を購読行に持たせ、UI で「要再登録」を出す
  （Phase 4 の任意項目）。

### 2.7 通知 UX（Service Worker）

- `notificationclick`:
  - 既存ウィンドウがあれば `client.navigate(target)` してから `focus()`
    する（`navigate` 非対応環境では `postMessage` で SPA 側に遷移を依頼）。
  - 承認 / 拒否アクションは従来どおり SW から API を叩く。409 は成功扱い。
- `requireInteraction` は承認通知のみ true。質問・完了は false。
- 前面表示中でも通知は出す（サーバー側の Active 判定で抑制されるのが本線）。
  SW 側で「表示しない」判定はしない（`userVisibleOnly` の制約）。
- 解決済みの承認通知のキャンセルは、Web Push の制約上、同じ endpoint へ
  「cancel」ペイロードを送って SW が `getNotifications({ tag })` を閉じる方式が
  考えられる。`userVisibleOnly` との整合を検証してから採用する（Phase 4 の調査項目）。

### 2.8 API / 設定

- `PushSubscribeRequest` に `client_id` を追加。
- 購読管理（ハブのみ）:
  - `GET /api/v1/push/subscriptions`: 一覧（`id`, `device_name`, `client_id`,
    `created_at`, `active`）。endpoint / 鍵は返さない。
  - `DELETE /api/v1/push/subscriptions/{id}`: 削除。
- `HubSettings` に通知設定を追加し、既存の `PATCH /api/v1/settings/hub` で更新する。
  楽観ロック（revision）は既存方式を踏襲する。
- 設定項目（案）:

| キー                            | 型 / 値                                                    | 既定値        |
| :------------------------------ | :--------------------------------------------------------- | :------------ |
| `push_target`                   | `all` / `active_only` / `active_or_recent` / `most_recent` | `active_only` |
| `push_recent_secs`              | 整数（秒）                                                 | `600`         |
| `push_fallback`                 | `all` / `none`                                             | `all`         |
| `push_suppress_focused_session` | 真偽値                                                     | `true`        |
| `notify_permissions`            | 真偽値                                                     | `true`        |
| `notify_questions`              | 真偽値                                                     | `true`        |
| `notify_turn_finished`          | 真偽値                                                     | `true`        |

- `push_target` は open enum とし、未知値は保持する。送信時は通知を落とさない
  よう `all` として扱い、警告ログを残す（型レベルで既定値に丸めない）。
- Presence の TTL / ハートビート間隔は実装定数（設定にしない）。

### 2.9 UI

- `ui/src/lib/push.ts`:
  - `client_id` の生成・保存。
  - 既存購読の再登録（`client_id` の backfill）。
  - `disablePushNotifications()` でサーバー側の購読も削除する。
- `ui/src/lib/stores/sync.svelte.ts`:
  - WS 接続時に presence を送信。
  - `visibilitychange` / `focus` / `blur` / `pageshow` / `pagehide` を購読。
  - ハートビートの開始・停止（再接続時に再開）。
  - `setActiveSession` で `focused_session_id` を更新して presence を送る。
- 設定画面（ハブ設定）:
  - 「通知」セクション: 宛先ポリシー、recent 秒、fallback、表示中セッションの抑止、
    種別トグル。
  - 購読一覧: デバイス名、Active バッジ、作成日時、削除ボタン。
  - 文言は英語（`ui/messages/en.json`）、paraglide 経由。
- `ConnectionMenu`: 既存の Enable / Disable を維持。ハブ以外では非表示のまま。

---

## 3. 変更対象の一覧

| レイヤー       | 変更                                                                                                              |
| :------------- | :---------------------------------------------------------------------------------------------------------------- |
| `fxg-protocol` | `PushSubscribeRequest.client_id`、購読一覧型、通知設定型、`ClientWsMessage::Presence`、`HubSettings` 拡張         |
| `fxg-db`       | migration（`push_subscriptions.client_id`、`hub_settings` の通知設定列）、購読の一覧 / 削除クエリ、設定の読み書き |
| `fxg-hub`      | 通知トリガの修正（質問・完了）、Running 集合、`PresenceRegistry`、宛先選択、並列送信、購読 API、設定適用          |
| `ui`           | `client_id`、presence 送信、購読再登録 / 削除、通知設定 UI、購読一覧 UI、SW のタップ遷移と `requireInteraction`   |
| `docs`         | `docs/02` / `03` / `05` の責務・不変条件を更新（§6.5）                                                            |

型の正は `fxg-protocol` と生成済み bindings、スキーマの正は
`crates/fxg-db/migrations/` とする。本書には全文を複製しない。

---

## 4. 実装フェーズ

### Phase 1: 既存バグの修正（P0）

1. `hub.rs` の通知対象抽出に `ElicitationRequest` を追加する。
2. `running_sessions` を `HubState` に移し、起動時に Running セッションを seed する。
   1 イベント / バッチでも Running → Idle で 1 回だけ通知する。
3. 通知候補の抽出を純粋関数化し、テストを追加する。
4. `docs/05` §3.3 の記述を実装に合わせて修正する。

**完了条件**: 質問通知が届く。ターン完了通知が「Running を観測した後の
Idle / Error」でのみ 1 回届く。resync では届かない。

### Phase 2: client_id と presence（P1）

1. DB migration と購読レコード / API に `client_id` を追加する。
2. `ClientWsMessage::Presence` とハブ側 `PresenceRegistry` を追加する。
   - 追加・更新・切断削除・TTL 失効。
   - `active_clients()` と `most_recent_client()` を提供する。
3. バックエンド trait に presence 登録のフックを追加する（ノード側は no-op）。
4. UI に `client_id`、presence 送信、ハートビートを追加する。
5. 宛先選択を実装し、既定 `active_only` + `fallback = all` で動かす。
6. `active_only` / `active_or_recent` / `most_recent` / `all` のテストを追加する。

**完了条件**: 2 台の端末で、Active な端末だけに承認・質問・完了通知が届く。
Active が 0 台なら fallback で全端末に届く。`focused_session_id` が一致する
端末には届かない。

### Phase 3: 通知設定と購読管理（P1）

1. `hub_settings` に通知設定列を追加し、`HubSettings` / `UpdateHubSettingsRequest`
   と snapshot を拡張する。設定更新時に `PushService` と通知ルーティングへ即時反映する。
2. `GET /api/v1/push/subscriptions` と `DELETE /api/v1/push/subscriptions/{id}` を追加する。
   - ハブのみ。ローカルノードは `INVALID_STATE`。
   - 設定変更・購読削除を監査ログに記録する。
3. 設定 UI（通知ポリシー・種別トグル・購読一覧）を追加する。
4. `disablePushNotifications()` からサーバー購読も削除する。
5. bindings と i18n を更新する。

**完了条件**: UI からポリシーを切り替えると次の通知から反映される。
購読一覧から不要な端末を削除できる。削除後に Push が送られない。

### Phase 4: 配信の堅牢性と通知 UX（P2）

1. Push 送信にタイムアウトと上限付き並列を導入する。
2. 401 / 403 の扱いを整理し、診断カウンタとログを改善する。endpoint を丸ごと
   ログに出さない。
3. SW の `notificationclick` を修正し、既存ウィンドウを対象セッションへ遷移させる。
4. `requireInteraction` を承認通知のみに限定する。
5. （調査）解決済み承認通知のキャンセル方式を検証する。
6. （任意）購読行に最終成功 / 連続失敗を持たせ、UI に「要再登録」を表示する。

**完了条件**: 1 台の応答不能な endpoint が他の送信を遅らせない。通知タップで
該当セッションが開く。承認通知だけが残る。

### Phase 5: ドキュメントと検証

§6 の検証を通し、`docs/02` / `03` / `05` を更新する。実装ログを本書 §8 に追記する。

---

## 5. テスト計画

### ユニットテスト

- 通知候補の抽出:
  - `PermissionRequest` / `ElicitationRequest` / `Running → Idle` / `Running → Error`
  - 連続 `Idle`、`Idle` 単独、重複イベント（`inserted = false`）、resync
  - 別バッチに分かれた Running と Idle
- 宛先選択:
  - ポリシー全 4 種、fallback、Active 0 台、複数 Active、`most_recent`
  - TTL 失効、複数タブ（1 タブだけ focused）、切断直後
  - `focused_session_id` 一致による抑止
  - `client_id` なし購読の扱い
- 設定:
  - 既定値、revision 競合、未知の `push_target` の保持
- 購読 DB:
  - `client_id` 付き upsert、一覧、削除、既存行（NULL）の扱い
- SW:
  - payload 変換、タップ時の遷移先決定（純粋関数化できれば）

### 結合テスト

- `crates/fxg-hub/tests/` の既存ハーネスで、WS 接続 → presence → イベント投入 →
  送信対象の選択までを検証できる範囲で追加する。実 Push 送信は
  `PushService` を差し替え可能にするか、選択関数の単体テストで代替する。
- `apply_event_batch` を通した Running → Idle の 1 回通知。

### 手動確認マトリクス

| ケース                       | 期待                                                       |
| :--------------------------- | :--------------------------------------------------------- |
| PC Chrome + Android PWA      | Active な端末のみ受信。PC を blur するとスマホへ           |
| 両方バックグラウンド         | fallback で全端末（既定）                                  |
| 同じセッションを PC で表示中 | そのセッションの承認通知は PC に来ない                     |
| 承認バナーで Approve         | アプリを開かず承認。409 も成功                             |
| 通知タップ（アプリ起動済み） | 既存ウィンドウが該当セッションへ遷移                       |
| ハブ再起動                   | presence が空になり fallback。通知は落ちない               |
| 購読を UI から削除           | 以後届かない。一覧から消える                               |
| VAPID 鍵を再生成             | 403 が記録され、再登録の必要が分かる（恒久リトライしない） |
| ローカルノード直結           | Push 導線が出ない。設定にも通知セクションは出ない          |

---

## 6. 留意点・リスク

### 6.1 通知の見逃し

`active_only` は「いま見ている端末だけ」に届くため、端末を持ち替えた直後などに
見逃す可能性がある。`fallback = all` を既定にし、`active_or_recent` を選べるように
することで緩和する。既定ポリシーは実機で調整する。

### 6.2 複数 Active 端末

PC とスマホを同時に使うと両方が Active になる。`active_only` /
`active_or_recent` は両方に送る。1 台に絞りたい場合だけ `most_recent` を選ぶ。
この差は設定 UI の説明文に明記する。

### 6.3 Presence の精度

- 画面ロック / スリープで `visibilitychange` が来る保証はない。
- モバイルのバックグラウンドでは WS が維持されたままになり得る。
- 対策: ハートビート + TTL + サーバー受信時刻。必要なら「最後のユーザー操作
  からの経過」も併用する。
- 「通知が来ない」より「来すぎる」を許容するなら `active_or_recent` を既定にする。

### 6.4 ローカルノード

Push はハブ専用のままとする。ローカルノード直結時は通知が無いことを
`docs/05` に明記する。デスクトップ通知が必要になった場合は別計画とする。

### 6.5 ドキュメント

- 更新対象: `docs/02`（`client_id` と presence の責務。DDL は migrations を正とする）、
  `docs/03`（`presence` の意味、購読 API、通知設定）、
  `docs/05`（通知ポリシー、制約、ローカルノード）。
- 実装のコピーは置かず、責務・不変条件・境界を書く。

### 6.6 セキュリティ

- `client_id` は認証ではない。他人の ID を騙っても通知先が変わるだけで、
  データへの権限は変わらない（単一ユーザー前提）。将来のマルチユーザーでは
  購読をユーザーにスコープする。
- endpoint は秘密を含む URL として扱い、ログ・API 応答に出さない。
- 購読削除・設定変更は監査ログに記録する。

### 6.7 過剰な後方互換を避ける

- UI / ハブ / プロトコルは同時に更新する。`client_id` は必須とし、
  旧形式の購読は再登録で移行する。
- `push_target` の未知値だけは、設定ファイル互換のため保持し、
  送信時に明示的に `all` 扱いとする（理由をログに残す）。

---

## 7. 実装順（コミット案）

1. `fix(fxg-hub): deliver question and turn-finished notifications reliably`
2. `feat(fxg-protocol,fxg-db): add push client identity and notification settings`
3. `feat(fxg-hub): target pushes at active clients`
4. `feat(ui): report client presence and register push clients`
5. `feat(ui,fxg-hub): manage notification settings and push subscriptions`
6. `fix(fxg-hub,ui): harden push delivery and notification taps`
7. `docs: document notification targeting and limits`

Phase 1 は独立して出せる。Phase 2 以降は migration と bindings の更新を含むため、
`mise run sqlx:prepare` と `mise run types` を各コミットで同期する。

---

## 8. 実装ログ / チェックリスト

### 2026-11-13 Phase 1

- `hub.rs` の通知対象抽出を `push::notification_events` に集約し、
  `ElicitationRequest` を配線した。
- ターン完了の Running 追跡を `HubState` の `RunningTurns` に移した。
  起動時に `hub.db` の Running セッションから seed するため、別バッチに
  分かれた Running → Idle / Error とハブ再起動をまたいでも 1 回だけ通知する。
  resync は状態だけ更新し、通知しない。`Stopped` / `SessionDeleted` は
  通知せず追跡から外す。
- `docs/05` §3.3 に質問・ターン完了通知の実装を追記した。

### 2026-11-13 Phase 2

- `push_subscriptions.client_id`（migration `0022_push_client_id.sql`）と
  `PushSubscribeRequest.client_id` を追加した。
- `ClientWsMessage::Presence` とハブの in-memory `PresenceRegistry` を追加した。
  TTL 90 秒、WS 切断で削除、ハブ再起動後は空（フォールバック）。
- UI は `localStorage` の `client_id` とタブごとの `tab_id` を送り、
  `visibilitychange` / `focus` / `blur` / `pageshow` / `pagehide` と 30 秒
  ハートビートで presence を更新する。Service Worker の再購読用に
  IndexedDB へ `client_id` をミラーする。
- 宛先選択を `select_subscriptions` に実装した。既定は `active_only` +
  `fallback_all` + 表示中セッションの抑止。`active_or_recent` /
  `most_recent` は Phase 3 の設定 UI から選択する。
- 既存購読は UI 起動時の再登録で `client_id` を backfill する。

- [x] Phase 1: 質問通知の配線
- [x] Phase 1: ターン完了通知の Running 追跡
- [x] Phase 1: 通知候補抽出のテスト
- [x] Phase 2: migration（`client_id`）
- [x] Phase 2: presence プロトコル / レジストリ
- [x] Phase 2: UI の presence 送信
- [x] Phase 2: 宛先選択とテスト
- [ ] Phase 3: 通知設定（DB / API / snapshot）
- [ ] Phase 3: 購読一覧・削除 API
- [ ] Phase 3: 設定 UI / i18n
- [ ] Phase 4: タイムアウト / 並列送信
- [ ] Phase 4: 通知タップ遷移 / `requireInteraction`
- [ ] Phase 5: docs 更新
- [ ] Phase 5: `mise run check` / 手動マトリクス

---

## 9. 未決事項

1. 既定の宛先ポリシーを `active_only` にするか `active_or_recent` にするか
   （本書は `active_only` + fallback `all` を推奨）。
2. 複数 Active 端末に送るか、`most_recent` を既定にするか。
3. ターン完了通知を既定で on にするか（本書は on）。
4. 解決済み承認通知のキャンセルを Phase 4 で実装するか、調査のみで留めるか。
