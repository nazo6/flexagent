import { createEffect, createMemo, createSignal, untrack } from "solid-js";
import { resolveFileHref } from "#lib/dock/file-links.ts";
import { enhanceCodeBlocks } from "#lib/highlight.ts";
import { renderMarkdown } from "#lib/markdown.ts";
import { dock } from "#lib/stores/dock.ts";
import { cn } from "#lib/utils.ts";
import "./markdown-text.css";

/**
 * Markdown renderer for chat messages and file previews.
 *
 * - Renders the output of `renderMarkdown` (`marked` + `DOMPurify`) as HTML.
 *   It is always rendered as Markdown, so content stays formatted while streaming.
 * - After rendering, `enhanceCodeBlocks` applies shiki highlighting to code
 *   fences asynchronously (debounced until the text settles).
 * - While streaming, re-rendering is coalesced to at most one markdown parse
 *   every [`STREAM_RENDER_INTERVAL_MS`]; agents that emit token-level deltas
 *   would otherwise reparse the whole message dozens of times per second.
 *   The finalized text is rendered immediately.
 */

/** Minimum spacing between markdown re-renders while the text is streaming. */
const STREAM_RENDER_INTERVAL_MS = 80;

export interface MarkdownTextProps {
  text: string;
  streaming?: boolean;
  class?: string;
  /** Directory for relative file links (defaults to the workspace root). */
  fileBaseDir?: string | null;
}

export default function MarkdownText(props: MarkdownTextProps) {
  const workspaceRoot = () => dock.context?.localPath ?? null;
  const fileBaseDir = () => props.fileBaseDir ?? workspaceRoot();
  const fileLinksEnabled = () => workspaceRoot() !== null;

  const [root, setRoot] = createSignal<HTMLDivElement>();
  /** Text currently converted to HTML (deliberately trails `text` while streaming). */
  const [renderedText, setRenderedText] = createSignal(untrack(() => props.text));
  /** Plain mirror of the signal, so the effect callback never reads it. */
  let renderedValue = untrack(renderedText);
  let lastRenderAt = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function render(next: string): void {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    if (next === renderedValue) return;
    renderedValue = next;
    setRenderedText(next);
    lastRenderAt = Date.now();
  }

  createEffect(
    () => ({ next: props.text, streaming: props.streaming === true }),
    (source) => {
      if (!source.streaming) {
        render(source.next);
        return;
      }
      if (timer !== null) return;
      // Leading edge stayed from the previous render, so only the remainder waits.
      const wait = Math.max(0, STREAM_RENDER_INTERVAL_MS - (Date.now() - lastRenderAt));
      timer = setTimeout(() => {
        timer = null;
        render(source.next);
      }, wait);
      return () => {
        if (timer !== null) {
          clearTimeout(timer);
          timer = null;
        }
      };
    },
  );

  const html = createMemo(() => {
    const body = renderMarkdown(renderedText());
    return props.streaming === true ? `${body}<span class="animate-pulse">▍</span>` : body;
  });

  // Re-apply shiki every time the HTML re-renders and rebuilds the DOM.
  // During streaming the DOM changes with each delta, so insert a short debounce
  // (unchanged code blocks are reused from the highlight.ts cache).
  createEffect(
    () => ({ node: root(), html: html() }),
    ({ node, html: current }) => {
      if (node === undefined || !current.includes("language-")) return;
      const highlightTimer = setTimeout(() => {
        void enhanceCodeBlocks(node);
      }, 150);
      return () => clearTimeout(highlightTimer);
    },
  );

  /** Opens a link to a workspace file in the Files dock instead of a new tab. */
  function openFileLink(event: MouseEvent): void {
    const base = fileBaseDir();
    const workspace = workspaceRoot();
    if (!fileLinksEnabled() || base === null || workspace === null) return;
    // Delegated `auxclick` also covers right-click; only middle-click opens.
    if (event.type === "auxclick" && event.button !== 1) return;
    const target = event.target;
    if (!(target instanceof Element)) return;
    const anchor = target.closest("a");
    if (anchor === null) return;
    const href = anchor.getAttribute("href");
    if (href === null) return;
    const resolved = resolveFileHref(href, base, workspace);
    if (resolved === null) return;
    event.preventDefault();
    dock.openFile(resolved.path, { line: resolved.line });
  }

  /** Event delegation survives the DOM rebuilds caused by the HTML binding. */
  const mountRoot = (element: HTMLDivElement): void => {
    element.addEventListener("click", openFileLink);
    element.addEventListener("auxclick", openFileLink);
    setRoot(() => element);
  };

  return <div class={cn("fxg-md", props.class ?? "")} ref={mountRoot} innerHTML={html()} />;
}
