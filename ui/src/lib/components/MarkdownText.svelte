<script lang="ts">
  /**
   * Markdown renderer for chat messages.
   *
   * - Renders the output of `renderMarkdown` (`marked` + `DOMPurify`) via `{@html}`.
   *   It is always rendered as Markdown, so content stays formatted while streaming.
   * - After rendering, `enhanceCodeBlocks` applies shiki highlighting to code
   *   fences asynchronously (debounced until the text settles).
   * - While streaming, re-rendering is coalesced to at most one markdown parse
   *   every [`STREAM_RENDER_INTERVAL_MS`]; agents that emit token-level deltas
   *   would otherwise reparse the whole message dozens of times per second.
   *   The finalized text is rendered immediately.
   */
  import { untrack } from "svelte";
  import { enhanceCodeBlocks } from "#lib/highlight.ts";
  import { renderMarkdown } from "#lib/markdown.ts";

  /** Minimum spacing between markdown re-renders while the text is streaming. */
  const STREAM_RENDER_INTERVAL_MS = 80;

  let {
    text,
    streaming = false,
    class: className = "",
  }: { text: string; streaming?: boolean; class?: string } = $props();

  let root: HTMLDivElement | undefined = $state(undefined);

  /** Text currently converted to HTML (deliberately trails `text` while streaming). */
  let renderedText = $state(untrack(() => text));
  let lastRenderAt = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function render(next: string): void {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    if (next === renderedText) return;
    renderedText = next;
    lastRenderAt = Date.now();
  }

  $effect(() => {
    const next = text;
    if (!streaming) {
      render(next);
      return;
    }
    if (timer !== null) return;
    // Leading edge stayed from the previous render, so only the remainder waits.
    const wait = Math.max(0, STREAM_RENDER_INTERVAL_MS - (Date.now() - lastRenderAt));
    timer = setTimeout(() => {
      timer = null;
      render(text);
    }, wait);
    return () => {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
    };
  });

  const html = $derived(renderMarkdown(renderedText));

  // Re-apply shiki every time `{@html}` re-renders and rebuilds the DOM.
  // During streaming the DOM changes with each delta, so insert a short debounce
  // (unchanged code blocks are reused from the highlight.ts cache).
  $effect(() => {
    const node = root;
    const current = html;
    if (node === undefined || !current.includes("language-")) return;
    const highlightTimer = setTimeout(() => {
      void enhanceCodeBlocks(node);
    }, 150);
    return () => clearTimeout(highlightTimer);
  });
</script>

<div class="fxg-md {className}" bind:this={root}>
  {@html html}{#if streaming}<span class="animate-pulse">▍</span>{/if}
</div>

<style>
  /* Markdown typography (theme variables and em units so it follows the parent font size) */
  :global(.fxg-md) {
    overflow-wrap: anywhere;
  }
  :global(.fxg-md > :first-child) {
    margin-block-start: 0;
  }
  :global(.fxg-md > :last-child) {
    margin-block-end: 0;
  }
  :global(.fxg-md p) {
    margin-block: 0.5em;
  }
  :global(.fxg-md h1),
  :global(.fxg-md h2),
  :global(.fxg-md h3),
  :global(.fxg-md h4),
  :global(.fxg-md h5),
  :global(.fxg-md h6) {
    margin-block: 1em 0.5em;
    font-weight: 600;
    line-height: 1.3;
  }
  :global(.fxg-md h1) {
    font-size: 1.35em;
  }
  :global(.fxg-md h2) {
    font-size: 1.2em;
  }
  :global(.fxg-md h3) {
    font-size: 1.1em;
  }
  :global(.fxg-md h4),
  :global(.fxg-md h5),
  :global(.fxg-md h6) {
    font-size: 1em;
  }
  :global(.fxg-md ul) {
    margin-block: 0.5em;
    padding-inline-start: 1.4em;
    list-style: disc;
  }
  :global(.fxg-md ol) {
    margin-block: 0.5em;
    padding-inline-start: 1.4em;
    list-style: decimal;
  }
  :global(.fxg-md li) {
    margin-block: 0.2em;
  }
  :global(.fxg-md li > p) {
    margin-block: 0.2em;
  }
  :global(.fxg-md li:has(> input[type="checkbox"])) {
    list-style: none;
  }
  :global(.fxg-md input[type="checkbox"]) {
    margin-inline-end: 0.4em;
    vertical-align: middle;
  }
  :global(.fxg-md :not(pre) > code) {
    border-radius: 0.3em;
    background: color-mix(in oklab, currentColor 8%, transparent);
    padding: 0.1em 0.35em;
    font-family: var(--font-mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace);
    font-size: 0.875em;
  }
  :global(.fxg-md pre) {
    margin-block: 0.6em;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--muted);
    padding: 0.65em 0.8em;
    overflow-x: auto;
    font-size: 0.85em;
    line-height: 1.6;
  }
  :global(.fxg-md pre code) {
    background: none;
    padding: 0;
    font-family: var(--font-mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace);
    font-size: inherit;
  }
  :global(.fxg-md blockquote) {
    margin-block: 0.6em;
    border-inline-start: 3px solid var(--border);
    padding-inline-start: 0.8em;
    color: var(--muted-foreground);
  }
  :global(.fxg-md hr) {
    margin-block: 1em;
    border: 0;
    border-top: 1px solid var(--border);
  }
  :global(.fxg-md a) {
    text-decoration: underline;
    text-underline-offset: 0.2em;
  }
  :global(.fxg-md a:hover) {
    opacity: 0.8;
  }
  :global(.fxg-md table) {
    display: block;
    margin-block: 0.6em;
    max-width: 100%;
    overflow-x: auto;
    border-collapse: collapse;
    font-size: 0.9em;
  }
  :global(.fxg-md th),
  :global(.fxg-md td) {
    border: 1px solid var(--border);
    padding: 0.3em 0.6em;
    text-align: start;
  }
  :global(.fxg-md th) {
    background: var(--muted);
    font-weight: 600;
  }
  :global(.fxg-md img) {
    max-width: 100%;
    border-radius: var(--radius-md);
  }
</style>
