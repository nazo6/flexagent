<script lang="ts">
  /**
   * チャットメッセージの Markdown 描画コンポーネント。
   *
   * - `renderMarkdown` (`marked` + `DOMPurify`) の出力を `{@html}` で描画する。
   *   常に Markdown として描画するため、ストリーミング中も整形済みで表示される。
   * - 描画後に `enhanceCodeBlocks` がコードフェンスへ shiki のハイライトを
   *   非同期適用する (テキストが落ち着くまでデバウンス)。
   */
  import { enhanceCodeBlocks } from '$lib/highlight';
  import { renderMarkdown } from '$lib/markdown';

  let {
    text,
    streaming = false,
    class: className = '',
  }: { text: string; streaming?: boolean; class?: string } = $props();

  let root: HTMLDivElement | undefined = $state(undefined);

  const html = $derived(renderMarkdown(text));

  // `{@html}` の再描画で DOM が作り直されるたびに shiki を適用し直す。
  // ストリーミング中はデルタごとに DOM が変わるため、短いデバウンスを挟む
  // (内容が同じコードブロックは highlight.ts のキャッシュで再利用される)。
  $effect(() => {
    const node = root;
    const current = html;
    if (node === undefined || !current.includes('language-')) return;
    const timer = setTimeout(() => {
      void enhanceCodeBlocks(node);
    }, 150);
    return () => clearTimeout(timer);
  });
</script>

<div class="fxg-md {className}" bind:this={root}>
  {@html html}{#if streaming}<span class="animate-pulse">▍</span>{/if}
</div>

<style>
  /* Markdown 要素のタイポグラフィ (テーマ変数と em 基準で親の文字サイズに追従) */
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
  :global(.fxg-md li:has(> input[type='checkbox'])) {
    list-style: none;
  }
  :global(.fxg-md input[type='checkbox']) {
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
