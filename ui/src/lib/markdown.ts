/**
 * チャットメッセージの Markdown レンダリング (`marked` + `DOMPurify`)。
 *
 * ACP エージェントの出力は外部由来のため、必ず `DOMPurify` でサニタイズして
 * から `{@html}` で描画する (生 HTML / イベントハンドラ / `javascript:` URL を除去)。
 * コードフェンスのシンタックスハイライトは `$lib/highlight` が描画後に付与する。
 *
 * 注意: `DOMPurify` は happy-dom では正しく動作しない (未サポート環境)。
 * テストは real DOM (jsdom / ブラウザ) で実行すること (`markdown.test.ts` 参照)。
 */

import DOMPurify from "dompurify";
import { marked } from "marked";

// リンクは常に新しいタブで開く (PWA のアプリ内遷移から外部へ逃がす)
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node instanceof HTMLAnchorElement && node.hasAttribute("href")) {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noopener noreferrer");
  }
});

/** Markdown テキストをサニタイズ済み HTML へ変換する。 */
export function renderMarkdown(text: string): string {
  const html = marked.parse(text, { async: false, gfm: true, breaks: true });
  return DOMPurify.sanitize(html);
}
