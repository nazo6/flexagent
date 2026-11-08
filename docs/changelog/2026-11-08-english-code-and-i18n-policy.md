# 2026-11-08 コードの英語化 (Phase 0 / Phase 1) と言語ポリシーの明文化

- **日付**: 2026-11-08
- **対象パッケージ**: `AGENTS.md`, `mise.toml`, `.github/workflows/ci.yml`, `fxg-protocol`
- **対象スクリプト**: `mise run lint:lang` / `mise run types` / `cargo test -p fxg-protocol` /
  `mise run fmt`

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

## 検証

- `mise run lint:lang` — 成功
- `cargo test -p fxg-protocol` — 44 件すべて成功
- `mise run types` — `bindings.ts` に日本語が残っていないことを確認
- `cargo fmt --all` / `tombi format` / `oxfmt` — 差分なし

## 今後の予定

| Phase    | 内容                                                                                                                                                                     |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 1 (完了) | `fxg-protocol` の英語化 + `bindings.ts` 再生成                                                                                                                           |
| 2        | 残りクレート (`fxg-db` → `fxg-pty` → `fxg-acp` → `fxg-server` → `fxg-node` → `fxg-cli`) のコメント英語化。クレートごとに 1 コミット、完了ごとに `lint:lang` の対象へ追加 |
| 3        | Rust のユーザー向け文字列 (CLI 出力・トレイ・Web Push・エージェントへ注入するプロンプト) の英語化。`TurnStopReason` 等は文言ではなくコードで渡す                         |
| 4        | UI 文字列を `paraglide-js` のメッセージカタログ (英語のみ) へ抽出し、`ErrorCode` → 文言マップを追加                                                                      |
| 5 (将来) | 日本語カタログの追加、トレイ / Web Push のロケール対応                                                                                                                   |
| 6        | 英語化の完了後に一時タスク `lint:lang` (タスク本体 / `lint` 集約 / CI ステップ) を削除する                                                                               |

Phase 1 で `fxg-protocol` を最初に選んだのは、`bindings.ts` (715 行) が自動で
英語化されるため、投資対効果が最も大きいからである。Phase 6 の削除を忘れないよう、
`lint:lang` の定義箇所には「一時的なタスク」である旨のコメントを残してある。
