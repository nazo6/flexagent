# 2026-11-08 コードの英語化 (Phase 0〜4)・一時タスクの削除 (Phase 6) と言語ポリシーの明文化

- **日付**: 2026-11-08
- **対象パッケージ**: `AGENTS.md`, `mise.toml`, `.github/workflows/ci.yml`,
  `fxg-protocol`, `fxg-db`, `fxg-pty`, `fxg-acp`, `fxg-server`, `fxg-node`, `fxg-cli`,
  `fxg-ui-assets`, `ui`
- **対象スクリプト**: `mise run lint:lang` / `mise run types` /
  `cargo test -p fxg-protocol` / `pnpm --dir ui run i18n:compile` /
  `pnpm --dir ui run check` / `mise run build:ui` / `mise run fmt`

## 背景

コードベースのコメント・doc コメント・CLI 出力・エラーメッセージ・UI 文言が日本語で書かれており、
以下の問題があった。

- コメントが日本語のため、コントリビュータや AI エージェントが文脈を読み取りにくい。
- `fxg-protocol` の doc コメントは `specta` 経由で `ui/src/lib/generated/bindings.ts`
  に出力されるため、UI 側の型ドキュメントまで日本語になっていた。
- 将来の UI 多言語化 (i18n) を見据えたときに、「どの文字列がユーザー向けか」が
  コード上で区別されていない。

## 方針

文字列を 3 種類に分けて扱う。

1. **コメント** — 英語化のみ。i18n の対象外。
2. **開発者向けメッセージ** (`thiserror` の `#[error]`、`tracing` ログ、`bail!` 等) —
   英語固定。ローカライズしない (ログの検索性を優先)。
3. **エンドユーザー向けメッセージ** — 英語をデフォルトとしつつ、後から翻訳を足せる形にする。
   - API エラーは既存の `ErrorCode` を i18n キーとして扱い、`message` は英語の
     フォールバックとして残す (サーバーにロケール交渉を持ち込まない)。
   - UI は `paraglide-js` を前提に、英語のみのメッセージカタログを先に用意する。
   - CLI / トレイ / Web Push は英語固定とし、翻訳が必要になった段階で個別に対応する。

`docs/` と `docs/changelog/` は日本語のまま維持する。

## 変更内容

### 1. 言語ポリシーの明文化 (`AGENTS.md`)

「4. 開発ルール・ワークフロー」に言語ポリシーを追加した。

- コメント・ログ・CLI 出力・エラーメッセージ・UI 文言は英語。
- `docs/` は日本語。ただしコードは英語。
- 意図的に日本語を残す行は `// allow-japanese: <理由>` を付ける。
- 英語化はクレート単位の独立したコミットで行う (挙動変更と混ぜない)。
- UI の多言語化は `paraglide-js`、API エラーは `ErrorCode` を i18n キーとする。

### 2. 非英語文字列の検出タスク (`mise run lint:lang`) — 移行期間限定

- `mise.toml` に `lint:lang` タスクを追加し、`mise run lint` の先頭で実行するようにした。
- **このタスクは移行期間限定の一時的なもの**であり、英語化が完了した時点で
  以下をまとめて削除する。
  1. `mise.toml` の `lint:lang` タスク本体
  2. `lint` 集約タスクからの `lint:lang` の参照
  3. `.github/workflows/ci.yml` の `mise run lint:lang` ステップ
  - 「コードは英語で書く」という規約自体は `AGENTS.md` に残す
    (タスクが無くなっても規約は有効)。
- 現時点の検出対象は**英語化が完了したパス**のみ (`crates/fxg-protocol`)。
  クレートの英語化が終わるたびに対象を追加する。
- 検出パターンは漢字・ひらがな・カタカナに加え、CJK 記号と全角形
  (`、。「」（）：` 等) も含む。
- `allow-japanese` を含む行は除外する。
- `rg` (PCRE2) を優先し、無い環境では `grep -P` にフォールバックする。

### 3. `fxg-protocol` の英語化 (Phase 1)

`crates/fxg-protocol` の全モジュール (約 5,100 行) のコメント・doc コメントを英語化した。

- `lib.rs` / `util.rs` / `export_bindings.rs`
- `common.rs` / `events.rs` / `ipc.rs` / `config.rs` / `client_api.rs` / `node_server.rs`
- テスト用のダミー文字列 (`"テストを修正して"` 等) も英語のプレースホルダに置換した。
- `crates/fxg-protocol/Cargo.toml` の `description` も英語化した。

型の意味・シリアライズ形式・`serde` / `specta` 属性は一切変更していない
(純粋なコメントとテストデータの置換)。

### 4. 型定義の再生成

`mise run types` を実行し、`ui/src/lib/generated/bindings.ts` を再生成した。
`specta` が doc コメントを JSDoc として出力するため、同ファイル 715 行の
日本語 JSDoc が英語に置き換わった (生成物は直接編集していない)。

### 5. 残りクレートの英語化 (Phase 2)

`fxg-protocol` 以外の全クレートのコメント・doc コメント・ログ・エラーメッセージ・
テストデータを英語化した。

- `fxg-db` / `fxg-pty` / `fxg-acp` / `fxg-server` / `fxg-node` / `fxg-cli` /
  `fxg-ui-assets` (計画には無かったが、Cargo.toml と Rust ソースを含むため同じ基準で
  対象に加えた)
- 各 `Cargo.toml` の `description` も英語化した。
- Phase 3 で扱う予定だったユーザー向け文字列 (CLI 出力・トレイ・Web Push・
  エージェントへ注入するプロンプト・ACP の turn-stop 文言) も、今回は**文字列の
  置換のみ**先行して英語化した。`TurnStopReason` を文言ではなくコードで渡す構造変更は
  Phase 3 に残す (挙動は変えていない)。
- UTF-8 のマルチバイト境界・CJK 検索など、日本語であること自体がテスト対象の行は
  日本語のまま残し、`// allow-japanese: <理由>` を付けた
  (`fxg-db` 13 行、`fxg-acp` 5 行、`fxg-node` 2 行、`fxg-cli` 1 行)。
- `fxg-db/src/registration.rs` の `sqlx::query!` 内の SQL コメントを英語化したため、
  `.sqlx/` のクエリキャッシュ 1 件を再生成した (クエリのメタデータは同一)。

### 6. マイグレーション SQL を対象外にした理由

`crates/fxg-db/migrations/*.sql` は英語化していない。`sqlx::migrate!` は適用済み
マイグレーションのチェックサムを検証するため、コメントだけでも書き換えると
既存の `node.db` / `server.db` が起動時に失敗する。`lint:lang` でも
`crates/fxg-db/migrations/**` を除外し、理由をタスク内のコメントに残した。
今後追加するマイグレーションは英語で書く。

### 7. `lint:lang` の対象拡大

`crates/fxg-protocol` に加え、上記 7 クレートを検出対象に追加した。

### 8. `TurnEnded` を理由コードのみに変更 (Phase 3)

- `UnifiedEventPayload::TurnEnded` から `message: Option<String>` を削除し、
  `reason: TurnStopReason` のみにした。`message` は ACP ドライバが `reason` に
  対応する英文を入れていた重複データで、UI / CLI は `reason` からラベルを導出できる。
- `fxg-acp` の `turn_ended_event()` は ACP `StopReason` を `TurnStopReason` へ
  変換するだけにした。
- UI (`ui/src/lib/sync/reducer.ts`) は常に `turnStopReasonLabel(reason)` を使うようにし、
  エージェント供給メッセージを優先するテストを削除した。
- FTS (`fxg-db` の `searchable_text`) では `TurnEnded` を対象外にした
  (理由コードは検索語として有用でないため)。
- `mise run types` で `bindings.ts` を再生成した。

### 9. UI のメッセージカタログ化 (Phase 4)

- `paraglide-js` 3.2.1 を導入し、inlang プロジェクト (`ui/project.inlang/settings.json`)
  を追加した。`baseLocale` / `locales` は `en` のみ。
- UI の全ユーザー向け文字列を `ui/messages/en.json` (556 キー) へ抽出した。
  生成物 `ui/src/lib/paraglide/` は Git 管理外とし、`pnpm --dir ui run i18n:compile`
  で再生成する (`check` 実行時に自動で走る)。
- `ui/vite.config.ts` に `paraglideVitePlugin` を追加した (`strategy: ["baseLocale"]`)。
- `ErrorCode` → 英語文言マップ `ui/src/lib/api/error-messages.ts` を追加した。
  `commandErrorMessage` と `CommandResult` のトーストは
  `message ?? errorCodeMessage(code) ?? <fallback>` の順で表示する。
- UI のコメントも英語化した。`ui/src/app.html` の `lang` を `en` に、
  `ui/static/manifest.webmanifest` の説明も英語にした。
- Service Worker (`ui/src/service-worker/index.ts`) は paraglide を読み込まず、
  通知文言を英語リテラルで持つ。Web Push のロケール対応は Phase 5 で扱う。
- `oxlint` の paraglide ルール (`no-dynamic-message-access`) を追加し、
  `ui/.oxlintrc.paraglide.json` を `pnpm --dir ui run lint` から実行する。
- `mise run lint:lang` の対象に `ui/src`, `ui/scripts`, `ui/static`, `ui/messages`,
  `ui/*.ts` を追加した。日本語を残したのはマルチバイトの base64 テスト 1 行のみ
  (`// allow-japanese`)。

### 10. 一時タスク `lint:lang` の削除 (Phase 6)

英語化が完了したため、移行期間限定だった非英語検出ガードを削除した。

- `mise.toml` から `lint:lang` タスク本体を削除し、`lint` 集約タスクからの参照も外した。
- `.github/workflows/ci.yml` から `mise run lint:lang` ステップを削除し、
  ジョブ名から `language` を外した。
- `AGENTS.md` から「`lint:lang` は一時的なタスク」という記述を削除した。
  「コメント・ログ・CLI 出力・エラーメッセージ・UI 文言は英語」「意図的に日本語を
  残す行には `// allow-japanese: <理由>`」という規約自体は残している。
- Phase 5 (日本語カタログの追加、トレイ / Web Push のロケール対応) は今回は
  スキップし、任意の TODO として残す。

## 検証

- `mise run lint:lang` — Phase 6 でタスクごと削除済み (Phase 2〜4 時点では成功)
- `cargo clippy --workspace --all-targets -- -D warnings` — 成功
- `cargo test --workspace` — 全件成功
- `pnpm --dir ui run i18n:compile` / `check` / `lint` / `test` — 成功 (65 件)
- `mise run build:ui` — 成功
- `mise run fmt:check` — 差分なし
- Phase 1 時点の検証:
  - `cargo test -p fxg-protocol` — 44 件すべて成功
  - `mise run types` — `bindings.ts` に日本語が残っていないことを確認

## 今後の予定

| Phase      | 内容                                                                                                                                                                                                                           |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 1 (完了)   | `fxg-protocol` の英語化 + `bindings.ts` 再生成                                                                                                                                                                                 |
| 2 (完了)   | 残り全クレート (`fxg-db` → `fxg-pty` → `fxg-acp` → `fxg-server` → `fxg-node` → `fxg-cli` → `fxg-ui-assets`) のコメント・メッセージ・ユーザー向け文字列の英語化。クレートごとに 1 コミット、完了ごとに `lint:lang` の対象へ追加 |
| 3 (完了)   | ユーザー向け文字列のうち、`TurnStopReason` 等を文言ではなくコードで渡す構造変更。`TurnEnded` の `message` を削除し、UI / CLI が `reason` から文言を導出する                                                                    |
| 4 (完了)   | UI 文字列を `paraglide-js` のメッセージカタログ (英語のみ) へ抽出し、`ErrorCode` → 文言マップを追加                                                                                                                            |
| 5 (未着手) | 日本語カタログの追加、トレイ / Web Push のロケール対応 (任意の TODO)                                                                                                                                                           |
| 6 (完了)   | 一時タスク `lint:lang` (タスク本体 / `lint` 集約 / CI ステップ) と `AGENTS.md` の一時タスク記述を削除する                                                                                                                      |

Phase 1 で `fxg-protocol` を最初に選んだのは、`bindings.ts` (715 行) が自動で
英語化されるため、投資対効果が最も大きいからである。Phase 6 の削除時に、
`lint:lang` の定義箇所に残していた「一時的なタスク」のコメントもまとめて削除した。
