<script lang="ts">
  import type { FileDiff } from '$lib/generated/FileDiff';
  import MonacoDiff from './MonacoDiff.svelte';
  import ShikiDiff from './ShikiDiff.svelte';

  let { file }: { file: FileDiff | null } = $props();

  /** PC (>= 768px) は Monaco Diff Editor、モバイルは shiki Unified Diff。 */
  let viewportWidth = $state(typeof window === 'undefined' ? 0 : window.innerWidth);

  function updateViewportWidth() {
    viewportWidth = window.innerWidth;
  }

  $effect(() => {
    if (typeof window === 'undefined') return;
    window.addEventListener('resize', updateViewportWidth);
    return () => window.removeEventListener('resize', updateViewportWidth);
  });

  const canUseMonaco = $derived(
    viewportWidth >= 768 && file !== null && file.old_text !== null && file.new_text !== null
  );
</script>

{#if file === null}
  <p class="text-muted-foreground flex h-full items-center justify-center text-sm">
    ファイルを選択してください。
  </p>
{:else}
  <div class="flex flex-col gap-2">
    <div class="flex flex-wrap items-center gap-2 text-xs">
      <span class="font-mono break-all">{file.path}</span>
      <span class="text-emerald-600 dark:text-emerald-400">+{file.additions}</span>
      <span class="text-red-600 dark:text-red-400">-{file.deletions}</span>
    </div>
    {#if canUseMonaco && file !== null}
      <MonacoDiff {file} />
    {:else if file.unified_diff !== ''}
      <ShikiDiff diff={file.unified_diff} />
    {:else}
      <p class="text-muted-foreground text-sm">
        このファイルはバイナリまたは差分テキストを取得できない形式です。
      </p>
    {/if}
  </div>
{/if}
