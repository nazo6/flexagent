# HTTP（非セキュアオリジン）での基本機能の縮退動作

- **日付**: 2026-11-19
- **対象パッケージ**: `ui`, docs
- **対象スクリプト**: `pnpm --dir ui run test`, `pnpm --dir ui run check`, `mise run fmt`
- **関連ドキュメント**: [`docs/07-web-ui-and-pwa.md`](../../07-web-ui-and-pwa.md) §5.1

## 背景

Web UI は `fxg web` が開く `localhost` だけでなく、LAN / Tailscale の
`http://<address>` でも利用されます。ブラウザは非セキュアオリジンで
Secure Context 限定 API を公開しないため、次の基本操作が失敗していました。

- メッセージ・セッション情報・ペアリングコマンド・リクエスト ID などの
  「コピー」操作（`navigator.clipboard` が `undefined`）
- Notification API が存在しない環境で、通知パネルが権限を「拒否」と誤表示

Service Worker / PWA インストール / Web Push はブラウザ仕様上 HTTP では
提供できないため、機能を制限したまま基本機能だけを動作させる方針にします。

## 変更

- `ui/src/lib/clipboard.ts` を追加。`navigator.clipboard.writeText()` が
  無い・拒否された場合は、一時的な textarea と `document.execCommand("copy")`
  にフォールバックする（`execCommand` は非推奨だが、ユーザー操作起点の
  コピーは全ブラウザで動作する）。成功可否は従来どおり呼び出し側の
  `catch` でローカライズ済みトーストとして表示する。
- すべてのコピー箇所（チャット、ProblemCenter、SessionInfo、Node tokens、
  Pairing、Settings catalog）を `copyText()` に統一。Settings catalog は
  未処理だったコピー失敗もトーストで通知するようにした。
- `ui/src/lib/push.ts`:
  - `Notification` が無い環境の権限を `"denied"` ではなく `"default"` として返す。
  - 非セキュアオリジンで `enablePushNotifications()` が呼ばれた場合は、
    「HTTPS / localhost が必要」というメッセージで失敗させる
    （UI 側は従来どおりボタンを無効化する）。
- UUID のフォールバック (`ui/src/lib/uuid.ts`) は既存のまま利用する。

## 注意（変更しない不変条件）

- サーバー側の認証・Host / Origin 検証・Cookie 属性は変更しない。
  `fxg_session` Cookie は `HttpOnly; SameSite=Strict` で、`Secure` 属性を
  付与しないため HTTP でもログインできる。HTTP は LAN / VPN 内に限定する。
- Service Worker / Web Push / PWA インストールは HTTP では引き続き利用できない。
  これらが必要な場合は TLS リバースプロキシで HTTPS 配信するか、
  `localhost` で Web UI を開く。
- コピーはクリックなどのユーザー操作を起点に実行する。自動実行のコピーは
  非セキュアオリジンでは失敗し得る（フォールバックもユーザー操作を要求する）。

## 検証

- `ui/src/lib/clipboard.test.ts`: 非同期 API の利用、API 不在時 / 拒否時の
  `execCommand` フォールバック、両方失敗時の例外を検証。
- `pnpm --dir ui run test`（216 tests）
- `pnpm --dir ui run check`（svelte-check / service worker tsc）
