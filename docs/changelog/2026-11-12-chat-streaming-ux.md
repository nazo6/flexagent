# チャットのストリーミング表示と思考ブロックのUX改善

- **日付**: 2026-11-12
- **対象パッケージ**: `ui` (`fxg-ui-assets` の埋め込みビルドに反映)
- **対象スクリプト / 設定**: `ui/vitest.config.ts`,
  `ui/messages/en.json` (paraglide メッセージ),
  `ui/src/lib/sync/timeline.svelte.ts`, `ui/src/lib/sync/reducer.ts`,
  `ui/src/lib/stores/sync.svelte.ts`,
  `ui/src/lib/components/MarkdownText.svelte`,
  `ui/src/lib/components/chat/ChatTimeline.svelte`,
  `ui/src/lib/components/chat/ThoughtBlock.svelte` (新規),
  `ui/src/routes/sessions/[id]/+page.svelte`

---

## 1. 症状と原因

セッションの応答が「ストリーミングではなく、完了時に一気に表示される」ように
見えていた。隔離したノードで受信時刻と描画時刻を計測したところ、
**サーバー側は 23 チャンクを 100–300ms 間隔で送っていた**が、画面は
「最初の 1 チャンク」と「確定イベント到着時の全文」の 2 回しか更新されて
いなかった。

原因は Svelte 5 の `$state` の Proxy 範囲にある。`$state` が深く Proxy 化する
のは **plain object と配列だけ**で、`Map` / `Set` は素通しになる
(`svelte/src/internal/client/proxy.js` の `proxy()` が
`prototype !== object_prototype && prototype !== array_prototype` で早期 return
する)。`SessionTimeline` の `messageDeltas` / `thoughtDeltas` / `toolProgress`
/ `terminalDeltas` は `$state(new Map())` だったため、`Map.set()` が
`$derived` を無効化せず、ストリーミング本文は再描画されなかった。
`streamKeys` だけは配列なので反応し、「最初の delta で項目が生える →
確定イベントで全文に置き換わる」という現在の見え方になっていた。

## 2. 変更内容

### 2.1 ストリーミングバッファを `SvelteMap` にする (`ui`, バックエンド変更なし)

- `SessionTimeline` の 6 バッファを `svelte/reactivity` の `SvelteMap` に変更。
- `SyncStore.pendingPrompts` / `provisioningLogs` も同じ理由で `SvelteMap` に
  (Pending Queue の出現・消滅と一時VMのログ行が即時反映される)。
- `#timelines` は同一性キャッシュでありリアクティブ状態ではないため plain
  `Map` とし、コメントで区別を明示。
- 副次効果として、ツール実行中の `raw_output` と `agent_terminal` の出力も
  ライブで伸びるようになった。

### 2.2 回帰テスト (`ui/vitest.config.ts`, `timeline-reactivity.test.svelte.ts`)

- Vitest の `resolve.conditions` に `browser` を追加。サーバービルドの
  `$effect` / `flushSync` は no-op のため、これを指定しないとリアクティビティ
  テストが「何も観測しないまま成功」してしまう。
- `include` に `*.test.svelte.ts` を追加し、`$effect.root` + `flushSync` で
  「delta を適用するたびに `buildTimelineItems` が再評価される」ことを固定。
  `$state(new Map())` に戻すとこのテストは失敗する (最初のチャンクだけしか
  観測できない) ことを確認済み。
- `reducer.test.ts` に、思考の経過時間 (`startedAt` / `endedAt`) の計測を追加。

### 2.3 思考ブロック (`ThoughtBlock.svelte`)

- ストリーミング中は自動展開し、確定したら自動で折りたたむ
  (`userToggled` / `userIsOpen` の既存イディオムを `ToolCallGroup` と共通化)。
- ヘッダーに経過時間 (`8s` / `1m 5s`) と、折りたたみ時の先頭行プレビューを表示。
- 本文は `max-h-64` + 内部スクロールで、長い思考が会話全体を押し下げない
  ようにし、ストリーミング中は内側だけ末尾追従する。
- 経過時間はクライアント側の観測値 (`startedAt` = 最初の delta、
  `endedAt` = 確定イベント)。履歴から復元した思考には時刻がないため表示しない。

### 2.4 見え方の調整

- `MarkdownText`: ストリーミング中の markdown 再パースを 80ms 間隔に合流。
  トークン単位の delta を送るエージェントでも 1 フレームあたりの作業量が
  跳ねない。確定時は即時描画する。
- `ChatTimeline`: 末尾が「ストリーミング中の本文 / 思考」または「実行中の
  ツール」のときは汎用の「処理中」行を出さない (各ブロックが自前の進捗表示を
  持つため二重になる)。
- `routes/sessions/[id]/+page.svelte`: 追従スクロールを「末尾から 80px 以内に
  いるとき」だけに限定し、`requestAnimationFrame` で 1 フレーム 1 回に合流。
  自分の発言を送信したときは末尾へ戻る。過去を読んでいる間は新しい出力で
  位置を奪わない。

## 3. 検証

- `pnpm --dir ui run check` / `lint` / `test` (118 tests) 通過。
- 実ブラウザでの計測 (`WebSocket` 受信時刻 vs `.fxg-md` の DOM 更新時刻):
  修正前は本文 13 delta に対して描画 2 回 (最初の 304 文字 + 確定時の一括)、
  修正後は delta ごとに 1 回ずつ伸びることを確認。
- 思考ブロックはストリーミング開始で `aria-expanded=true`、確定イベントで
  `false` に戻ること、ヘッダーが `Thinking process 8s <先頭行>` になることを
  実ブラウザで確認。

## 4. 今後の余地

- 文字単位のスムージング表示は今回のスコープ外。現在のチャンク粒度は
  ACP エージェント側の送出間隔 (100–300ms) に依存する。
- 長大なセッションでは `buildTimelineItems` が delta ごとに全イベントを
  走査する。必要になったら「永続イベント部分」と「ストリーミング部分」に
  分割して delta あたりの再構築を定数化する。
