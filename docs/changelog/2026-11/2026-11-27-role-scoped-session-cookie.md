# Web UI セッション Cookie のロール別分離と永続化

- **日付**: 2026-11-27
- **対象パッケージ**: `fxg-api`, `fxg-protocol`, `fxg-node`, `fxg-hub`, `ui`
- **対象スクリプト**: `mise run types`, `cargo test -p fxg-api`,
  `cargo test -p fxg-node --lib daemon::http::tests`,
  `cargo test -p fxg-hub --test meta_endpoint`

## 背景

Web UI の認証 Cookie は `fxg_session` という単一の名前で、`Max-Age` を
指定しないブラウザセッション Cookie だった。Cookie はホスト単位で
**ポートを区別しない**ため、同じホストで hub (`:8080`) と local node
(`:7860`) を開くと互いの Cookie を上書きする。`fxg session new` が node の
Web UI を `?token=` 付きで開くワークフローでは、hub のログインがその都度
失われ、認証ダイアログが繰り返し表示されていた。またブラウザ終了のたびに
再ログインが必要だった。

サーバー側の Web アクセストークンには有効期限がなく、再起動でも
`<data>/<role>/auth_token` を再利用する。したがって失効の原因は
Cookie のスコープと寿命にあった。

## 設計

- Cookie 名をロール別にする: `fxg_session_node` / `fxg_session_hub`。
  認証ミドルウェアは接続先ロールの Cookie だけを読む。
- Cookie を永続化する: `Path=/; Max-Age=2592000`（30 日）。
  `HttpOnly; SameSite=Strict` は維持する。
- ログイン・トークン再生成・ログアウトの `Set-Cookie` 生成を
  `fxg-api::session` に集約し、属性の不一致を防ぐ。
- 旧 `fxg_session` は移行しない（旧 Cookie は一度だけ再ログインになる）。
  互換用のフォールバックは置かない。
- サーバー側のセッション期限は導入しない。失効はトークン再生成のみ。

## 影響

- 既存ブラウザは初回アクセス時に一度だけトークン入力が必要。
- 同じホストで hub と node を行き来しても、互いのログインを上書きしない。
- ブラウザ再起動後も 30 日間はログインが維持される（Cookie を消した場合を除く）。
