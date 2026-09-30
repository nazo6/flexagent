<script lang="ts">
  import DiffText from './DiffText.svelte';
  import { highlightUnifiedDiff } from '$lib/highlight';

  let { diff, maxHeight = '60svh' }: { diff: string; maxHeight?: string } = $props();

  let html = $state<string | null>(null);

  const prefersDark =
    typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches;

  $effect(() => {
    const text = diff;
    let disposed = false;
    html = null;
    void (async () => {
      try {
        const rendered = await highlightUnifiedDiff(text, prefersDark);
        if (!disposed) html = rendered;
      } catch {
        // ハイライト失敗時は DiffText へフォールバックする
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
    class="overflow-auto rounded-md border text-xs [&_code]:block [&_code]:min-w-max [&_pre]:p-2 [&_pre]:font-mono [&_pre]:leading-relaxed"
    style={`max-height:${maxHeight}`}
  >
    {@html html}
  </div>
{:else}
  <DiffText {diff} {maxHeight} />
{/if}
