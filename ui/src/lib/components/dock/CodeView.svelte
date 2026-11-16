<script lang="ts">
  import { languageForPath } from "#lib/code-languages.ts";
  import { FOCUS_LINE_CLASS, highlightCodeBlock, prefersDarkMode } from "#lib/highlight.ts";

  let { path, text, line = null }: { path: string; text: string; line?: number | null } = $props();

  let html = $state<string | null>(null);
  let scroller = $state<HTMLDivElement | null>(null);

  $effect(() => {
    const currentPath = path;
    const currentText = text;
    const currentLine = line;
    let disposed = false;
    html = null;
    void (async () => {
      try {
        const rendered = await highlightCodeBlock(
          currentText,
          languageForPath(currentPath),
          prefersDarkMode(),
          currentLine,
        );
        if (!disposed) html = rendered;
      } catch {
        if (!disposed) html = null;
      }
    })();
    return () => {
      disposed = true;
    };
  });

  // Center the requested line without scrolling the surrounding page. Shiki
  // marks it with `FOCUS_LINE_CLASS` while building the highlighted HTML.
  $effect(() => {
    const node = scroller;
    const current = html;
    if (node === null || current === null) return;
    const frame = requestAnimationFrame(() => {
      const target = node.querySelector<HTMLElement>(`.${FOCUS_LINE_CLASS}`);
      if (target === null) return;
      const targetRect = target.getBoundingClientRect();
      const nodeRect = node.getBoundingClientRect();
      node.scrollTop += targetRect.top - nodeRect.top - (node.clientHeight - targetRect.height) / 2;
    });
    return () => cancelAnimationFrame(frame);
  });
</script>

{#if html !== null}
  <div
    bind:this={scroller}
    class="h-full overflow-auto rounded-md border text-[12px] [&_code]:block [&_code]:min-w-max [&_pre]:w-max [&_pre]:min-w-full [&_pre]:p-2 [&_pre]:font-mono [&_pre]:leading-relaxed [&_.fxg-line-focus]:inline-block [&_.fxg-line-focus]:w-full [&_.fxg-line-focus]:bg-signal-soft/60"
  >
    {@html html}
  </div>
{:else}
  <pre
    class="bg-surface-raised h-full overflow-auto rounded-md border p-2 font-mono text-[12px] leading-relaxed"><code
      >{text}</code
    ></pre>
{/if}
