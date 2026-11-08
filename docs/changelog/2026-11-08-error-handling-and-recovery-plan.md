# エラー処理・診断性・復旧性の改善プラン

- **日付**: 2026-11-08
- **対象パッケージ**: `fxg-protocol`, `fxg-db`, `fxg-pty`, `fxg-acp`,
  `fxg-node`, `fxg-server`, `fxg-cli`, `fxg-ui-assets`, `ui`
- **対象スクリプト / 設定**: `Cargo.toml`（workspace lints）, `clippy.toml`（新規）,
  `mise.toml`, `.github/workflows/ci.yml`, `ui/package.json`,
  `ui/.oxlintrc.json`, `ui/src/lib/generated/bindings.ts`（生成物）,
  `docs/troubleshooting.md`（新規）

---

## 1. 背景と目的

現状は「エラーが起きたこと自体」はログに出るが、**ユーザーが原因を理解し、
その場で復旧するための情報と手段が不足している**。主な症状は次の 3 つ。

1. **握り潰し**: `Result` を `let _ =` / `.ok()` / `unwrap_or_default()` で捨て、
   状態だけが変わって UI には成功に見える箇所がある
2. **原因の解像度不足**: `NodeError::error_code()` がほぼすべてを
   `ErrorCode::Internal` に潰し、UI は「Internal error」しか出せない
3. **復旧手段の不在**: エラーはトーストで消え、再試行・再開・別ノード・
   診断情報の取得といった「次の一手」が提示されない

本プランの目的は、**「なぜ起きたかを即座に理解でき、必要ならその場で復旧できる」
状態を、プロトコル・バックエンド・UI・運用ツールの全層で実現すること**。

### 設計原則

- **Fail loud, recover fast**: 失敗は必ず観測でき、次の一手が同時に提示される
- **One error, one code, one action**: 同じ原因は同じコード、コードごとに
  復旧手順が決まる
- **No silent degradation**: 一部だけ壊れた状態も「劣化」として表示する
- **Explain before act**: 失敗する前に事前検証（preflight）で予告する
- **Recovery is safe**: 復旧操作は冪等で、データを壊さない
- **Errors are data, not strings**: 自由文ではなく構造化された
  `ErrorInfo` を境界で受け渡す

---

## 2. 現状の主要課題（調査サマリ）

テストを除く Rust コードには `let _ =` が 179 箇所、`.ok()` が 33 箇所ある。
多くは正常な後始末だが、以下は**ユーザーに見える誤動作・データ損失・誤診断**に
つながる。

### 2.1 重大（誤動作・データ損失）

| 場所                                                        | 内容                                                 | 実害                                       |
| :---------------------------------------------------------- | :--------------------------------------------------- | :----------------------------------------- |
| `fxg-acp/src/acp.rs:1325-1334`                              | `PromptResponse` の `Err` を warn 後に `Idle` を送る | ターン失敗が正常終了に見える               |
| `fxg-node/src/session_manager.rs:495-501, 820-827, 888-896` | resume / fork の履歴 Replay 注入失敗を warn のみ     | 「引き継いで再開」と表示されるが文脈が空   |
| `fxg-node/src/session_manager.rs:1929-1938`                 | スナップショット失敗を debug のみで `None` にする    | 後から revert 不能。理由が分からない       |
| `fxg-node/src/project.rs:189`                               | `.fxg.toml` 読み取り失敗を空文字扱いして上書き       | 既存設定の消失                             |
| `fxg-cli/src/main.rs:88`, `commands.rs:416`                 | `GlobalConfig::load(...).unwrap_or_default()`        | 設定破損が無言でデフォルト動作になる       |
| `fxg-db/src/queries.rs:446, 774, 818`                       | JSON 列の parse 失敗を空/default に                  | エージェント未導入・承認選択肢消失の誤表示 |
| `fxg-server/src/state.rs:213-218`                           | `list_nodes().await.ok()?`                           | DB 障害が「オンラインノードなし」に化ける  |
| `fxg-node/src/daemon/stdio.rs:133-141`                      | stdin の読み取りエラーを EOF 扱い                    | 一時VMノードが無言終了                     |
| `fxg-acp/src/acp.rs:826`                                    | 書き込み前の旧内容読み取り失敗を空文字に             | Diff が全行追加として誤表示                |
| `fxg-cli/src/commands.rs:763-767`                           | `token.unwrap_or_default()`                          | 空パスワードで Git 認証が失敗              |

### 2.2 中程度（誤診断・復旧困難）

| 場所                                                                 | 内容                                               |
| :------------------------------------------------------------------- | :------------------------------------------------- |
| `fxg-node/src/daemon/ops.rs:588-601`                                 | worktree 探索のエラーを握り潰して `not found`      |
| `fxg-node/src/bootstrap.rs:517-521`, `credentials.rs:248-252`        | `chmod` 失敗を無視                                 |
| `fxg-cli/src/service.rs:147, 180, 183, 368, 372`                     | service 操作の失敗を無視して成功表示               |
| `fxg-node/src/daemon/mod.rs:742-746`, `fxg-server/src/server.rs:138` | `let _ = task.await` でタスクの Err / panic を無視 |
| `fxg-node/src/sync.rs:188-190`, `fxg-server/src/hub.rs:1000`         | WS 受信エラーを正常切断扱い                        |
| `fxg-server/src/api/ws.rs:69-74, 175-179`                            | replay 失敗理由をクライアントへ返さず切断          |
| `fxg-node/src/daemon/mod.rs:456-464`                                 | installed_agents 取得失敗が debug のみ             |
| `fxg-node/src/session_manager.rs:1941-1954`                          | git 失敗で branch / worktree 情報が欠損            |

### 2.3 「なぜ」を説明できない構造

- `fxg-node/src/error.rs:103-120` の `_ => ErrorCode::Internal` により、
  Agent / Git / Io / Config / Json の区別が失われる
- `ApiErrorBody` は `{ code, message }` のみ、`CommandResult` も
  `{ code, error }` のみで、`details` / `retryable` / 復旧アクション /
  相関 ID を持たない
- `SystemInfoResponse` / `SystemStatus` は同期状態のみで、コンポーネントの
  健全性（DB / IPC / registry / agent spawn / disk）を持たない
- ヘルスチェック（`/healthz` / `/readyz`）が存在しない
- エージェントの stderr は `debug` ログのみ（`fxg-acp/src/acp.rs:1058-1063`）
- エラー履歴（インシデント）が永続化されず、リモート UI から参照できない

### 2.4 フロントエンド

- `ui/src/lib/stores/sync.svelte.ts:501-538`: WS メッセージの parse 失敗・
  未知 op を無言で捨てる
- `NewSessionChat.svelte:112-123`: provisioner / agents 取得失敗を空配列扱い
- `TerminalView.svelte:44-49`: PTY 入力の送信失敗を無視
- `service-worker/index.ts:241-253`: Push 購読 POST の失敗を確認しない
- 空 `catch` が点在し、共通のエラーレポート経路がない

---

## 3. フェーズ一覧

| フェーズ    | 対象領域                                       | 主な対象パッケージ                             | 依存                  |
| :---------- | :--------------------------------------------- | :--------------------------------------------- | :-------------------- |
| **Phase 1** | エラー契約・相関ID・静的解析ガード             | `fxg-protocol`, 全クレート, `mise.toml`, CI    | なし                  |
| **Phase 2** | 握り潰しの修正（高リスク経路）                 | `fxg-acp`, `fxg-node`, `fxg-cli`, `fxg-db`     | Phase 1（一部並行可） |
| **Phase 3** | 診断基盤（インシデント・ヘルス・ログ・doctor） | `fxg-db`, `fxg-node`, `fxg-server`, `fxg-cli`  | Phase 1               |
| **Phase 4** | 復旧アクション・部分成功・UI エラー提示        | `fxg-protocol`, `fxg-node`, `fxg-server`, `ui` | Phase 1, 3            |
| **Phase 5** | 事前検証・互換性チェック・安全な再試行         | 全クレート, `ui`                               | Phase 1, 3            |
| **Phase 6** | メッセージ品質・i18n・ドキュメント・契約テスト | `fxg-protocol`, `ui`, `docs`, CI               | Phase 1〜5            |

各フェーズは独立したコミット / PR に分割する。AGENTS.md のとおり、
**英語化・大規模リファクタと挙動変更を同じコミットに混ぜない**。

---

## Phase 1: エラー契約・相関ID・静的解析ガード

- **目的**: 「捨てる」「Internal に潰す」を機械的に防ぎ、後続フェーズが使う
  共通のエラー表現を確定する。

### タスクリスト

- [x] `fxg-protocol`: `ErrorCode` を拡張する
      (`ConfigInvalid`, `AgentNotInstalled`, `AgentLaunchFailed`,
      `AgentProtocolError`, `GitUnavailable`, `GitFailed`, `WorktreeConflict`,
      `SnapshotUnavailable`, `SyncBacklog`, `DbCorrupt`, `ProvisionerFailed`,
      `VersionMismatch`, `Timeout`, `DiskFull`, `PermissionDenied`, `IoError`,
      `PtyFailed`, `ServerError`, `Cancelled` を追加)
- [x] `fxg-protocol`: `ErrorInfo` / `RecoveryAction` / `WarningInfo` /
      `DataSafety` 型を追加する
- [x] `fxg-protocol`: `ApiErrorResponse` / `CommandResult` /
      `ServerWsMessage::Error` / `ServerWsMessage::CommandResult` /
      `NodeToServerMsg::CommandResult` / IPC のエラー結果を `ErrorInfo`
      ベースへ統一する
- [x] `fxg-node`: `NodeError::error_code()` を**ワイルドカードなしの
      網羅マッチ**にし、Io / Git / Agent / Config / Db / Pty などを
      `INTERNAL` から分離する - `Agent(String)` / `Server(String)` の stage + source 構造化は
      Phase 2 の ACP エラー改善と同時に行う
- [x] `fxg-server`: `ApiError` に `details` / `retryable` / `recovery` /
      `request_id` を追加し、`ErrorCode` → HTTP status の写像を更新する
- [x] `fxg-db`: JSON 列の decode 失敗を `DbError::InvalidPayload` として
      返す（lint 有効化の前提として先行修正）
- [x] `clippy.toml` を追加し、`mise run lint:errors` で
      `fxg-protocol` / `fxg-db` に `let_underscore_must_use` /
      `map_err_ignore` / `disallowed_methods` を適用する - Cargo は `[lints] workspace = true` とクレート単位 override の併用を
      許さないため、workspace の `all = "warn"` は維持し、
      `disallowed_methods` は `allow` のまま dedicated task の `-D` で
      上書きする方式に変更した
- [x] `mise run types` で `bindings.ts` を同期し、UI の型・エラー表示
      (`ApiError` / `CommandResult` / `ServerWsMessage::Error`) を
      `ErrorInfo` に対応する
- [x] `cargo clippy --workspace --all-targets -- -D warnings` と
      `mise run lint:errors` が通る
- [ ] 残り: `request_id` の REST ミドルウェア採番とログ span 伝播
      （Phase 3 の診断基盤と同時に実装する）
- [x] `discard_with` / `spawn_logged` マクロを `fxg-protocol::util` に追加
      （既存の `let _ =` / `tokio::spawn` の移行は Phase 2 で行う）
- [ ] 残り: `lint:errors` の対象を `fxg-acp` → `fxg-node` → `fxg-server` →
      `fxg-cli` へ拡大する

### 完了条件 / 検証

- [x] `cargo clippy --workspace --all-targets -- -D warnings` が通る
- [x] `cargo clippy -p fxg-protocol -p fxg-db --all-targets -- -D warnings
      -D clippy::let_underscore_must_use -D clippy::map_err_ignore
      -D clippy::disallowed_methods` が通る
- [x] `mise run types` 後に `ui` の `svelte-check` が通る
- [x] すべての `NodeError` variant が `Internal` 以外のコードに写ることを
      コンパイル時に保証する
- [x] `cargo test --workspace --no-fail-fast` が通る

### 実装ログ / 進捗メモ

- **2026-11-08**: ブランチ `feat/error-handling-recovery` を切り、
  本プランを `docs(plan)` でコミットしてから Phase 1 に着手。
- **プロトコル**: `ErrorCode` を 30 値に拡張し、`ErrorInfo` /
  `RecoveryAction` / `WarningInfo` / `DataSafety` を追加。エラー本文は
  REST・Client WS・Node⇔Server・IPC で `ErrorInfo` に統一した。
  `CommandResult::failure` は `command_id` を `request_id` として補完する。
- **NodeError**: `error_code()` を網羅マッチに変更し、`Io` の
  `PermissionDenied`、`Config`、`Db` の破損系、`Pty`、`Git`、`Agent` を
  それぞれ専用コードへ写像するようにした。
- **ApiError**: `details` / `retryable` / `recovery` / `request_id` を追加。
  `result_large_err` を避けるため `details` と `request_id` は Box 化した。
- **UI**: `ApiError` が `ErrorInfo` を保持するようにし、`CommandResult`
  利用箇所を `result.error?.code` / `commandErrorMessage(result.error)` へ
  更新。新しい `ErrorCode` には paraglide の英語メッセージを追加した。
- **静的解析**: `clippy.toml` の `disallowed-methods` で
  `Result::ok` / `Result::unwrap_or_default` を禁止。Cargo の制約により
  クレート単位 override が使えないため、`mise run lint:errors` で
  `-p fxg-protocol -p fxg-db` に限定して `-D` を渡す方式とした。
  `fxg-db` の JSON decode は先に `InvalidPayload` へ修正し、
  `map_err_ignore` は整数変換の理由付き `allow` で明示した。
- **検証**: `fxg-protocol` 49 テスト、`fxg-node` の `sync_e2e` 4 テスト、
  workspace の clippy / test、UI の svelte-check が通ることを確認。
  既存 E2E の `CommandResult` 期待値を `error.code` へ更新した。
- **共通ヘルパ**: `discard_with!` / `spawn_logged!` マクロを
  `fxg-protocol::util` に追加した（呼び出し側の移行は Phase 2）。
- **先送り**: REST の `request_id` ミドルウェアと他クレートへの lint 展開は
  Phase 2 / 3 で実施する。

---

## Phase 2: 握り潰しの修正（高リスク経路）

- **目的**: 「動いているように見えて実は壊れている」経路をなくす。
  各修正に回帰テストを付ける。

### A. セッション・データの整合性

- [ ] ACP ターンエラーを `StatusChanged(Error)` + ターン失敗イベントとして
      表面化する（`fxg-acp/src/acp.rs:1325-1334`）
- [ ] resume / fork の Replay 注入失敗を `Err` または `WarningInfo` として返す
      （`fxg-node/src/session_manager.rs:495-501, 820-827, 888-896`）
- [ ] スナップショット失敗を診断イベント / 応答の warning に含める
      （`session_manager.rs:1929-1938`）
- [ ] `.fxg.toml` の読み取りは `NotFound` のみ空扱いにし、他は伝播する
      （`fxg-node/src/project.rs:187-213`）
- [ ] DB の JSON parse 失敗を `DbError::InvalidPayload` として返す
      （`fxg-db/src/queries.rs:446, 774, 818`）
- [ ] 書き込み前の旧ファイル読み取り失敗を差分の誤表示にしない
      （`fxg-acp/src/acp.rs:826`）

### B. 起動・設定・外部コマンド

- [ ] `GlobalConfig::load` の失敗を fail-fast（または明示 warn）にする
      （`fxg-cli/src/main.rs:88`, `commands.rs:416`）
- [ ] stdio の stdin エラーと EOF を区別する
      （`fxg-node/src/daemon/stdio.rs:133-141`）
- [ ] git-askpass で token が無い場合に明示エラーを返す
      （`fxg-cli/src/commands.rs:763-767`）
- [ ] askpass スクリプトの `chmod` 失敗を伝播する
      （`bootstrap.rs:517-521`, `credentials.rs:248-252`）
- [ ] worktree 探索のエラーを `not found` と区別して返す
      （`daemon/ops.rs:588-601`）
- [ ] service 操作（systemctl / schtasks / launchctl）の失敗を
      成功表示しない（`fxg-cli/src/service.rs`）
- [ ] `select_agent_node` の DB エラーを空リストに変換しない
      （`fxg-server/src/state.rs:213-218, 1076-1080`）

### C. 通信・バックグラウンドタスク

- [ ] WS 受信エラーを正常切断扱いせず、backoff 対象の `Err` にする
      （`fxg-node/src/sync.rs:188-190`, `fxg-server/src/hub.rs:1000`）
- [ ] Client WS の replay 失敗時に `ServerWsMessage::Error` を送ってから閉じる
      （`fxg-server/src/api/ws.rs:69-74, 175-179`）
- [ ] バックグラウンドタスクを `spawn_logged` へ移行し、`JoinError` / panic を
      必ずログに出す（`fxg-node/src/daemon/mod.rs:742-746`,
      `fxg-server/src/server.rs:138`）
- [ ] `installed_agents` の取得失敗を warn 以上にし、NodeHello の結果を
      「劣化」として扱う（`fxg-node/src/daemon/mod.rs:456-464`）
- [ ] `branch_and_worktree` の git 失敗を `Result` で返す
      （`session_manager.rs:1941-1954`）
- [ ] UI: WS parse 失敗・未知 op を `reportError` に流す
      （`ui/src/lib/stores/sync.svelte.ts:501-538`）
- [ ] UI: PTY 入力送信失敗・WS error を状態として表示する
      （`TerminalView.svelte`）
- [ ] Service Worker: Push 購読 POST のレスポンスを検証する
      （`ui/src/service-worker/index.ts:241-253`）

### 完了条件 / 検証

- 上記それぞれに、失敗時に「成功扱いにならない」ことを確認する回帰テストを追加
- 意図的に握り潰す箇所は `discard_with("理由")` または
  `#[allow(..., reason = "...")]` で明示されている
- `mise run check` が通る

---

## Phase 3: 診断基盤（インシデント・ヘルス・ログ・doctor）

- **目的**: リモートのノードでも「何が起きたか」をユーザーと運用者が
  取得できるようにする。

### タスクリスト

- [ ] `fxg-db`: `incidents` テーブルを追加する
      （`id`, `created_at`, `level`, `code`, `summary`, `details_json`,
      `session_id`, `node_id`, `request_id`, `source`, `resolved_at`。
      上限付きリングバッファ + 保持期間）
- [ ] `fxg-node` / `fxg-server`: エラー境界でインシデントを記録する
      （同一原因の連続はグルーピング）
- [ ] コンポーネントヘルスを導入する
      （`db` / `ipc` / `http` / `central sync` / `registry` / `agent spawn` /
      `disk` / `migrations`。各 `ok` / `degraded` / `error` + 最終エラー +
      最終成功時刻）
- [ ] `GET /healthz`（liveness）と `GET /readyz`（DB / マイグレーション込み）
      を追加する
- [ ] `SystemInfoResponse` / `SystemStatus` にヘルスと劣化理由を追加する
- [ ] エージェント stderr のセッション別リングバッファを実装する
      （直近 N 行 + 終了コード + 最後の ACP メッセージを
      セッション診断に含める）
- [ ] 同期・配信のドロップカウンタを実装する
      （`dropped_events` / `lagged_broadcasts` / `replay_failures` /
      `outbox_retries` / `push_failures` / `stale_registry_served`）
- [ ] `GET /api/v1/diagnostics` と
      `GET /api/v1/diagnostics/incidents` を追加する
- [ ] `fxg doctor` を追加する
      （設定検証、DB 整合性、ディスク、バージョン、直近インシデント、
      コンポーネントヘルス）
- [ ] `fxg status --verbose` / `fxg logs` / `fxg session diagnose <id>` を追加する
- [ ] サポートバンドル出力（`fxg diagnostics export` / UI）を追加する - バージョン、設定（秘密情報マスク）、DB 統計、インシデント、
      ログ末尾、ノード / セッション状態 - **トークン・Cookie・環境変数・プロンプトの redaction を必須にする**

### 完了条件 / 検証

- 障害を注入したとき、`/api/v1/diagnostics` と `fxg doctor` に原因と
  タイムスタンプが現れる
- サポートバンドルに秘密情報が含まれないことをテストで確認する
- `/readyz` が DB / マイグレーション異常時に 503 を返す

---

## Phase 4: 復旧アクション・部分成功・UI エラー提示

- **目的**: エラー表示を「通知」から「復旧の入口」へ変える。

### タスクリスト

- [ ] 各エラー境界で `RecoveryAction` を生成する
      （エージェント未導入 → インストール / 別ノード、
      停止セッション → Replay で再開、ノードオフライン → 再接続 / 別ノード、
      worktree 競合 → 既存を開く / ブランチ変更、スナップショット欠落 →
      直近の復元地点へ revert 等）
- [ ] 部分成功を標準化する - API 応答 / イベントに `warnings: Vec<WarningInfo>` を追加 - `success | partial | failed` の 3 状態を導入 - 例: fork 作成成功 + Replay 失敗、resume 成功 + snapshot なし
- [ ] `DataSafety` を応答に含める
      （「送信されていない」「ワークスペースは未変更」「backup tree は X」
      「Outbox に残り自動再送」）
- [ ] UI に `reportError(context, error)` と診断ストアを追加する
- [ ] `window.addEventListener("unhandledrejection" / "error")` と
      ルート単位のエラーバウンダリを追加する
- [ ] 共通 `ErrorBanner` コンポーネントを作る
      （メッセージ / 復旧ボタン / 詳細アコーディオン /
      相関 ID コピー / ログを見る）
- [ ] 永続的な問題はトーストではなくバナーで表示する
      （ノードオフライン、同期遅延、設定不正、DB 劣化）
- [ ] 空・エラー・読み込み中を別状態として描画する
      （エージェント一覧、プロジェクト、worktree、Inbox）
- [ ] `NewSessionChat` の provisioner / agents 取得失敗をエラー表示にする
- [ ] 失敗時にフォーム入力・下書きを保持する
- [ ] oxlint を強化する
      （`no-empty` の `allowEmptyCatch: false`、`no-floating-promises`、
      `no-misused-promises`。対応バージョンを確認して導入）

### 完了条件 / 検証

- 代表的な失敗シナリオで、UI に原因・データ安全・復旧ボタン・詳細が出る
- 復旧ボタンで実際に復旧できることをコンポーネント / E2E テストで確認する
- 空 `catch` が残っていない（意図的な fallback は `reportError` を呼ぶ）

---

## Phase 5: 事前検証・互換性チェック・安全な再試行

- **目的**: 失敗を「起きてから」ではなく「起きる前」に伝え、
  再試行・破壊的操作を安全にする。

### タスクリスト

- [ ] セッション作成 / worktree 作成 / 一時VM起動の preflight を実装する - ノードオンライン、プロトコル / DB スキーマのバージョン互換 - エージェントの導入状態と起動コマンド解決 - パスの存在・書き込み可否・git リポジトリ判定 - worktree ブランチの競合 - プロビジョナーの到達性・認証 - ディスク容量、マイグレーション状態、トークン有効期限
- [ ] `PreflightFailed { checks: [...] }` を返し、UI は該当コントロールを
      理由付きで無効化する
- [ ] `NodeHello` に node / server / DB スキーマのバージョンを載せ、
      不一致を `VERSION_MISMATCH` として明示する
- [ ] `retryable` / `idempotent` を全コマンドに付与し、
      UI の再試行を安全にする（`command_id` の再利用方針を定義）
- [ ] 破壊的操作の安全性を強化する - worktree 削除前の未コミット変更検出 - セッション削除の不可逆性を明示（タイトル付き確認） - DB マイグレーション前の自動バックアップ - revert 前の backup tree を応答に含める
- [ ] CLI に `--dry-run` / `--json` エラー出力を追加する

### 完了条件 / 検証

- 事前に分かる失敗は、開始後ではなく開始前に理由付きで返る
- バージョン不一致時に不可解な部分失敗ではなく明示エラーになる
- 復旧操作を二重実行しても状態が壊れない（冪等性テスト）

---

## Phase 6: メッセージ品質・i18n・ドキュメント・契約テスト

- **目的**: エラーを「説明可能」に保ち、再発を防ぐ。

### タスクリスト

- [ ] エラーメッセージの型を統一する
      （何が起きたか / なぜ / データは安全か / 次に何をすべきか / 詳細）
- [ ] `ErrorCode` を i18n キーとして扱い、`message` は英語フォールバックに
      統一する（paraglide-js。サーバーにロケール交渉を持ち込まない）
- [ ] `docs/troubleshooting.md` を ErrorCode ごとに作成する
      （よくある原因 / 確認コマンド / 復旧手順）
- [ ] UI の「詳細」から `docs/troubleshooting.md#<code>` へリンクする
- [ ] エラーカタログを生成し、契約テストを追加する - すべての `NodeError` variant が `Internal` 以外のコードに写る - すべての `ErrorCode` に翻訳・docs アンカー・復旧アクションがある - UI に生の内部エラー文字列が出ていない
- [ ] 障害注入テストを追加する
      （ターン中のノード kill、DB / キャッシュ破損、権限拒否、
      ディスク満杯、トークン失効、バージョン不一致）
- [ ] 復旧ドリルを追加する
      （オフライン→再同期、エージェント未導入→導入→再試行、
      worktree 競合→解消）
- [ ] AGENTS.md / 設計ドキュメントにエラー処理規約を追記する
      （`Result` を暗黙に捨てない、ユーザー影響は状態として表面化する、
      バックグラウンドタスクは `spawn_logged` を使う、など）

### 完了条件 / 検証

- 契約テストと障害注入テストが CI で通る
- 代表的なエラーについて、UI のメッセージ・復旧手順・docs が一致している
- `mise run check` が通る

---

## 4. 横断的な完了条件

- `mise run check`（fmt:check / lint / check:ui / test）が全フェーズ後に通る
- `mise run types` の生成物が最新で、`bindings.ts` を手編集していない
- 新しいエラー経路を追加したとき、`ErrorCode` の追加と
  `docs/troubleshooting.md` の追記が同じ PR に含まれている
- 「失敗が成功として表示される」経路が残っていないことを、
  レビュー観点として明文化する

---

## 5. 進め方・コミット方針

- 1 トピック 1 コミット（Conventional Commits）。例:
  - `feat(fxg-protocol): add structured ErrorInfo and recovery actions`
  - `fix(fxg-acp): surface prompt failures as session errors`
  - `feat(fxg-db): persist diagnostics incidents`
  - `feat(ui): add error banner with recovery actions`
- Phase 1 の lint 導入はクレート単位で分割し、挙動変更と混ぜない
- Phase 2 の修正は 1 修正 1 テストを原則とする
- Phase 3 以降は設計判断を本ファイルの「実装ログ」に追記する

### 実装ログ / 進捗メモ

- （未着手）

---

## 6. リスク・未決事項

- **互換性**: `ErrorInfo` への移行で `CommandResult` / `ApiErrorBody` の
  形が変わる。クライアント（CLI / UI）を同時に更新する前提とし、
  過剰な後方互換は持たない（AGENTS.md の方針）
- **ログ量**: インシデントと stderr リングバッファの保持上限・
  ローテーションを決める必要がある
- **redaction**: サポートバンドルに含める情報の範囲とマスク規則を
  先に決める（トークン、Cookie、環境変数、プロンプト本文、ファイルパス）
- **復旧アクションの実行主体**: サーバーが返すアクションを UI / CLI が
  どう実行するか（REST 再呼び出し / WS コマンド / CLI コマンド）の
  マッピングを Phase 4 で確定する
- **バージョン互換**: 常駐ノードと中央サーバーの更新順序を許容するため、
  `NodeHello` のバージョン交渉と拒否時のメッセージを明確にする

---

## 7. 参照

- [`docs/changelog/2026-09-30-implementation-plan.md`](2026-09-30-implementation-plan.md):
  Phase 1〜6 の実装プランと進捗
- [`docs/01-architecture-and-sync.md`](../01-architecture-and-sync.md):
  Outbox 同期・再同期・セキュリティ
- [`docs/03-protocol-and-api.md`](../03-protocol-and-api.md):
  プロトコル・REST / WS API・エラーコード
- [`docs/04-agent-drivers-and-windows.md`](../04-agent-drivers-and-windows.md):
  `AgentDriver`・ACP エラー・Shadow Git Revert
- [`docs/05-cli-and-pwa-ui.md`](../05-cli-and-pwa-ui.md):
  CLI コマンド・設定・PWA
