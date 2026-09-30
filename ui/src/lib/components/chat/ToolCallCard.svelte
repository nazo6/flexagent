<script lang="ts">
  import DiffText from '$lib/components/diff/DiffText.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import type { TimelineItem } from '$lib/sync/reducer';
  import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
  import CircleXIcon from '@lucide/svelte/icons/circle-x';
  import FileSearchIcon from '@lucide/svelte/icons/file-search';
  import HammerIcon from '@lucide/svelte/icons/hammer';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import SearchIcon from '@lucide/svelte/icons/search';
  import WrenchIcon from '@lucide/svelte/icons/wrench';

  let { item }: { item: Extract<TimelineItem, { kind: 'tool' }> } = $props();

  function statusVariant(status: string): 'secondary' | 'default' | 'destructive' | 'outline' {
    switch (status) {
      case 'completed':
        return 'secondary';
      case 'failed':
        return 'destructive';
      case 'in_progress':
        return 'default';
      default:
        return 'outline';
    }
  }
</script>

<div class="bg-card/60 flex flex-col gap-2 rounded-lg border p-2.5">
  <div class="flex flex-wrap items-center gap-2">
    {#if item.toolKind === 'read'}
      <FileSearchIcon class="text-muted-foreground size-4" />
    {:else if item.toolKind === 'edit'}
      <PencilIcon class="text-muted-foreground size-4" />
    {:else if item.toolKind === 'execute'}
      <HammerIcon class="text-muted-foreground size-4" />
    {:else if item.toolKind === 'search'}
      <SearchIcon class="text-muted-foreground size-4" />
    {:else}
      <WrenchIcon class="text-muted-foreground size-4" />
    {/if}
    <span class="text-sm font-medium">{item.title}</span>
    <Badge variant={statusVariant(item.status)}>
      {#if item.status === 'completed'}
        <CircleCheckIcon />
      {:else if item.status === 'failed'}
        <CircleXIcon />
      {:else if item.status === 'in_progress'}
        <LoaderCircleIcon class="animate-spin" />
      {/if}
      {item.status}
    </Badge>
    <span class="text-muted-foreground ml-auto font-mono text-xs">#{item.seq}</span>
  </div>

  {#if item.locations.length > 0}
    <ul class="text-muted-foreground flex list-none flex-col gap-0.5 p-0 font-mono text-xs">
      {#each item.locations as location (location)}
        <li class="truncate">{location}</li>
      {/each}
    </ul>
  {/if}

  {#if item.diff !== null}
    <DiffText diff={item.diff.unified_diff} maxHeight="14rem" />
  {/if}

  {#if item.rawOutput !== null && item.rawOutput !== ''}
    <details>
      <summary class="text-muted-foreground hover:text-foreground cursor-pointer text-xs select-none">
        出力
      </summary>
      <pre
        class="bg-muted/30 mt-1 max-h-48 overflow-auto rounded-md border p-2 font-mono text-xs whitespace-pre-wrap">{item.rawOutput}</pre>
    </details>
  {/if}
</div>
