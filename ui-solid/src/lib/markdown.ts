/**
 * Markdown rendering for chat messages (`marked` + `DOMPurify`).
 *
 * ACP agent output is externally sourced, so always sanitize it with
 * `DOMPurify` before rendering via `{@html}` (remove raw HTML / event handlers /
 * `javascript:` URLs). Syntax highlighting for code fences is applied after
 * rendering by `#lib/highlight.ts`.
 *
 * Note: `DOMPurify` does not work correctly in happy-dom (unsupported
 * environment). Run tests with a real DOM (jsdom / browser); see
 * `markdown.test.ts`.
 */

import DOMPurify from "dompurify";
import { marked } from "marked";

/**
 * DOMPurify's default URI allowlist, extended with `file:` URIs and Windows
 * drive / UNC paths so chat links to local files survive sanitization. Schemes
 * that can execute script (`javascript:`, `vbscript:`, `data:`) stay rejected.
 */
const ALLOWED_URI_REGEXP =
  /^(?:(?:(?:f|ht)tps?|mailto|tel|callto|sms|cid|xmpp|file):|[a-z]:(?:[\\/]|%5c|%2f)|[^a-z]|[a-z+.-]+(?:[^a-z+.\-:]|$))/i;

// Always open links in a new tab (escape the PWA's in-app navigation)
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node instanceof HTMLAnchorElement && node.hasAttribute("href")) {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noopener noreferrer");
  }
});

/** Convert Markdown text into sanitized HTML. */
export function renderMarkdown(text: string): string {
  const html = marked.parse(text, { async: false, gfm: true, breaks: true });
  return DOMPurify.sanitize(html, { ALLOWED_URI_REGEXP });
}
