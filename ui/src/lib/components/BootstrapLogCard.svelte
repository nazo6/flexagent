<script lang="ts">
  import { Badge } from '$lib/components/ui/badge';
  import WrenchIcon from '@lucide/svelte/icons/wrench';

  let { lines }: { lines: string[] } = $props();

  let open = $state(true);
  let scrollEl = $state<HTMLPreElement | null>(null);

  // 新しい行が届いたら末尾へスクロールする
  $effect(() => {
    if (lines.length > 0 && scrollEl !== null && open) {
      scrollEl.scrollTop = scrollEl.scrollHeight;
    }
  });
</script>

{#if lines.length > 0}
  <div class="bg-card/60 rounded-lg border">
    <button
      type="button"
      class="flex w-full items-center gap-2 px-3 py-2 text-left text-xs font-medium"
      onclick={() => (open = !open)}
    >
      <WrenchIcon class="size-3.5" />
      Environment Bootstrap Log
      <Badge variant="secondary">{lines.length} 行</Badge>
      <span class="text-muted-foreground ml-auto">{open ? '折りたたむ' : '展開する'}</span>
    </button>
    {#if open}
      <pre
        bind:this={scrollEl}
        class="bg-muted/30 max-h-48 overflow-auto border-t p-2 font-mono text-xs whitespace-pre-wrap">{lines.join('\n')}</pre>
    {/if}
  </div>
{/if}
