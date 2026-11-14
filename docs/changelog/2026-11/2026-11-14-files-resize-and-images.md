# Files の分割比調整と画像プレビュー

- **日付**: 2026-11-14
- **対象パッケージ**: `fxg-protocol`, `fxg-node`, `ui`
- **対象スクリプト / 設定**: `cargo test -p fxg-protocol -p fxg-node`,
  `pnpm --dir ui run check`, `pnpm --dir ui exec vitest run`,
  `pnpm --dir ui run lint`, `pnpm --dir ui run build`, `mise run types`,
  `mise run fmt`
- **関連ドキュメント**:
  - [`docs/03-protocol-and-api.md`](../../03-protocol-and-api.md) §3.1
  - [`docs/07-web-ui-and-pwa.md`](../../07-web-ui-and-pwa.md) §3.2
  - [`docs/proposals/2026-11-14-workspace-dock.md`](../../proposals/2026-11-14-workspace-dock.md) §3.2, §3.3

## 背景

Files のツリー幅は固定で、長いパスや広い画面ではプレビューが窮屈になることが
あった。また画像ファイルは `Binary` として扱われ、内容を確認できなかった。

## 変更

- **ツリー / プレビューの分割比をドラッグで変更**: デスクトップの Files に
  境界ハンドルを追加した。ドラッグ中はローカル state で追従し、離したときに
  workspace ごとの `treeWidth`（20–60%）として保存する。
- **画像プレビュー**: `WorkspaceFileContent` に `Image { mime, data_b64 }` を追加。
  ノードは拡張子が PNG / JPEG / GIF / WebP / BMP / AVIF / ICO のファイルを
  data URL 用の Base64 として返し、Files の `View` が `<img>` で表示する。
  SVG はスクリプトを含み得るため対象外とし、従来どおりテキスト表示にする。
- 画像も既存の 2 MiB 上限に従う。超過時は `TooLarge`、上限内でも画像以外の
  バイナリは `Binary` のまま。

## 不変条件

- 分割比は workspace ごとに保持し、セッション切替では変わらない。
- 画像として返すのは許可した拡張子のみ。内容判定ではなく拡張子で決めるため、
  拡張子を偽装したファイルはブラウザのデコード失敗として表示される。
- 画像の読み取りも workspace の access root と `.git` 拒否の検証を通る。

## 検証

- `fxg-node` に画像拡張子の MIME 対応と、PNG が `Image` として返るテストを追加。
- `dock.svelte.ts` に分割比の clamp と永続化のテストを追加した。
- `cargo test -p fxg-protocol -p fxg-node` / UI の check / lint / vitest / build が成功した。

## 既知の制約

- SVG / PDF / 動画はプレビューしない。
- 画像の diff はテキスト diff ではないため、`Diff` では内容を比較できない。
- 分割比のダブルクリックリセットやキーボード操作は未対応。
