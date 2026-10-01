# 送信時の自動レジューム (ネイティブ限定 / Auto Resume on Send)

- **日付**: 2026-10-01
- **対象パッケージ**: `fxg-protocol`, `fxg-acp`, `fxg-node`, `fxg-server`, `ui`
- **対象スクリプト**: `cargo test` / `cargo clippy` / `mise run fmt` /
  `mise run lint:ui` / `mise run test:ui`
- **状態**: ✅ 実装完了

---

## 0. ゴールとスコープ

停止済みセッションへプロンプトを送信したとき、**ネイティブ復元
(`session/resume` / `session/load` / opencode2 既存セッション bind) が
可能なエージェントについては、明示的な Resume 操作なしで自動的に会話を
継続**できるようにする。

| 項目         | 決定                                                                                    |
| ------------ | --------------------------------------------------------------------------------------- |
| トリガー     | 停止済みセッションへの `SendPrompt` (Web UI / REST / IPC)                               |
| 復元方式     | **ネイティブ限定**。不可なら新規作成せず `RESUME_REQUIRED` を返す                       |
| Replay       | **暗黙実行しない**。クライアントが「履歴を引き継いで再開して送信」を提案 (明示同意)     |
| 実行モデル   | ノード側で同期実行 (1 コマンド 1 応答)。全クライアントに一律適用                        |
| タイムアウト | コールドスタート (~25 秒) を許容するため、セッションコマンド中継を 30 秒 → 120 秒に延長 |
| 一時VM       | v1 同様に対象外 (`INVALID_STATE`)                                                       |

## 1. 背景

- セッション・レジューム (2026-10-01) により停止済みセッションの再開は可能に
  なったが、Web UI のコンポーザは停止中に入力できず、明示的な Resume
  ボタン/コマンドが必要だった (`docs/04 §4.3`)。
- 「ネイティブ復元できるならユーザー操作なしで続きから話せるべき」という
  要望への対応。Replay フォールバックは履歴を 1 ターンとして注入し
  エージェントがそれへ応答するため、暗黙実行は避け、ユーザーの明示操作に残す。

## 2. 設計判断

1. **ネイティブ限定はドライバの契約で表現する**: `ResumeRequest { allow_fresh:
   false }` で復元不可の場合、ドライバは `NativeResumeUnavailable` を返し
   新規セッションを作成しない (ACP は `plan_resume`、opencode2 は存在確認・
   未記録 ID の各分岐)。ノードはダウンキャストで判別し
   `NodeError::ResumeRequired` → `ErrorCode::RESUME_REQUIRED` に変換する。
2. **失敗しても停止状態を維持する**: 自動レジュームのネイティブ非対応失敗では
   `StatusChanged(Error)` を記録しない (既存の起動失敗と区別する)。
3. **排他**: 明示 `resume` と同一の `resuming` ガードで直列化する。他の再開が
   進行中の場合は 100ms 間隔で完了を待ち (上限 120 秒)、復帰後に送信する。
4. **プロトコル追加は ErrorCode のみ**: 既存の `SendPrompt` / `SessionResume`
   経路を再利用し、新イベント型・新メッセージ型は追加しない。
5. **UI は提案型**: `RESUME_REQUIRED` で入力を保持したまま
   「履歴を引き継いで再開して送信」を表示し、クリックで
   `SessionResume` (Replay フォールバック) → 再送する。

## 3. 変更点

### 3.1 `fxg-protocol`

- `ErrorCode::ResumeRequired` (`RESUME_REQUIRED`) を追加。
- ts-rs 出力 (`ui/src/lib/generated/ErrorCode.ts`) を再生成。

### 3.2 `fxg-acp`

- `NativeResumeUnavailable` エラー型を追加 (`driver.rs`、`thiserror`)。
- `AcpDriver::plan_resume`: `resume` 指定が `Some` かつ `agent_session_id` が
  `None` の場合に `allow_fresh` を尊重する (従来は常に Fresh)。
  `allow_fresh = false` なら `NativeResumeUnavailable`。
- `AcpDriver::build_session`: ネイティブ限定で `session/resume` /
  `session/load` 自体が失敗した場合も `NativeResumeUnavailable` に変換。
- `OpenCode2Driver::start_session`: セッション不存在・確認失敗・ID 未記録で
  `allow_fresh = false` なら `NativeResumeUnavailable`。

### 3.3 `fxg-node`

- `NodeError::ResumeRequired` を追加し `ErrorCode::ResumeRequired` へマップ。
- `SessionManager::resume_stopped_session(session_id, mode)` を追加し、明示
  resume と自動レジュームの検証・spec 解決・`StatusChanged(Idle)` を共通化
  (`ResumeMode::{AllowReplay, NativeOnly}`)。
- `SessionManager::send_prompt`: 非 active セッションは
  `auto_resume_for_prompt` でネイティブ限定復元してから送信。
- `start_driver_session`: `NativeResumeUnavailable` は `StatusChanged(Error)`
  を記録せず `ResumeRequired` を返す。
- `testutil::MockAgent`: ネイティブ限定契約 (`allow_fresh = false` で復元不可
  なら `NativeResumeUnavailable`) をエミュレート。

### 3.4 `fxg-server`

- `SESSION_COMMAND_TIMEOUT` (120 秒) を追加し、セッションコマンド中継
  (`hub.command`) と `hub.resume_session` に適用。
- `ApiError::from_code`: `RESUME_REQUIRED` → `409 Conflict`。

### 3.5 UI

- `Composer.svelte`: 停止中も入力・送信可能。停止バナーを
  「送信すると自動で再開します」に更新。
- `RESUME_REQUIRED` 受信時は入力を保持したまま
  「履歴を引き継いで再開して送信」ボタンを表示 (Replay 再開 → 再送)。
- **Resume ボタンは通常非表示**。`resumeRequired` をセッションページ
  (`ui/src/routes/sessions/[id]/+page.svelte`) が保持し、`Composer` へ
  `bind:` で共有する。ネイティブ復元非対応 (`RESUME_REQUIRED`) を検出した
  セッションでのみヘッダーの Resume ボタンを表示する (それ以外は送信時の
  自動レジュームで再開されるため不要)。Composer の停止バナーからは
  常設の Resume ボタンを削除。
- `sync.svelte.ts`: `sendPrompt` のみ `SEND_PROMPT_TIMEOUT_MS` (120 秒) を使用。

## 4. 検証

- `cargo test --workspace` /
  `cargo clippy --workspace --all-targets -D warnings`
- `fxg-node` 追加テスト:
  - `send_prompt_auto_resumes_stopped_session_natively`
    (ネイティブ限定復元 + 送信 + Replay 非注入 + idle 復帰)
  - `send_prompt_requires_explicit_resume_when_native_unsupported`
    (`RESUME_REQUIRED` / 停止状態維持 / 明示 resume で復旧)
- `fxg-acp` 追加テスト: `plan_resume` のネイティブ限定経路
  (`NativeResumeUnavailable` のダウンキャスト)。
- `mise run lint:ui` / `mise run test:ui` (58 tests) / `mise run fmt`

## 5. 実装ログ / 進捗メモ

- 2026-10-01: 実装完了。ACP / opencode2 双方で「ネイティブ復元不可」を
  `NativeResumeUnavailable` に統一し、ノードが `RESUME_REQUIRED` として
  クライアントへ返す経路を確立。
- 同期実行のため、停止済みセッションへの送信はクライアント WS 接続上で
  コールドスタート分 (最大 ~25 秒) 待つ。イベント配信の遅延はカーソル
  replay で欠落なく追従する (`fxg-server/src/api/ws.rs` の `Lagged` 処理)。
- 明示 Resume の REST 中継も 120 秒に延長 (コールドスタート時のタイムアウト
  を予防)。
- 2026-10-01 (追補): Resume ボタンを通常非表示化。`resumeRequired` を
  セッションページへ移して Composer と共有し、ネイティブ復元非対応を
  検出したときのみ表示するようにした。
