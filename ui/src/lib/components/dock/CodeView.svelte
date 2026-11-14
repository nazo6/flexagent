<script lang="ts">
  import { languageForPath } from "#lib/code-languages.ts";
  import { highlightCodeBlock, prefersDarkMode } from "#lib/highlight.ts";

  let { path, text }: { path: string; text: string } = $props();

  let html = $state<string | null>(null);

  $effect(() => {
    const currentPath = path;
    const currentText = text;
    let disposed = false;
    html = null;
    void (async () => {
      try {
        const rendered = await highlightCodeBlock(
          currentText,
          languageForPath(currentPath),
          prefersDarkMode(),
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
</script>

{#if html !== null}
  <div
    class="h-full overflow-auto rounded-md border text-[12px] [&_code]:block [&_code]:min-w-max [&_pre]:min-w-full [&_pre]:w-max [&_pre]:p-2 [&_pre]:font-mono [&_pre]:leading-relaxed"
  >
    {@html html}
  </div>
{:else}
  <pre
    class="bg-surface-raised h-full overflow-auto rounded-md border p-2 font-mono text-[12px] leading-relaxed"><code
      >{text}</code
    ></pre>
{/if}
