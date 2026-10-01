<script lang="ts">
  import { Badge } from '$lib/components/ui/badge';
  import { formatRelativeTime } from '$lib/format';
  import { connection, sync } from '$lib/stores/app.svelte';
  import { cn } from '$lib/utils';
  import WifiIcon from '@lucide/svelte/icons/wifi';
  import WifiOffIcon from '@lucide/svelte/icons/wifi-off';

  interface Props {
    class?: string;
  }

  let { class: className }: Props = $props();

  const isLocalNode = $derived(connection.systemInfo?.role === 'local_node');
</script>

<div class={cn('flex flex-wrap items-center gap-1.5 text-xs', className)}>
  <Badge
    variant={sync.wsConnected ? 'secondary' : 'destructive'}
    class="gap-1 px-1.5 py-0 text-[10px] font-normal"
  >
    {#if sync.wsConnected}
      <WifiIcon class="size-3 shrink-0" />
      <span>接続中</span>
    {:else}
      <WifiOffIcon class="size-3 shrink-0" />
      <span>切断</span>
    {/if}
  </Badge>

  {#if isLocalNode}
    <Badge
      variant={sync.centralConnected ? 'secondary' : 'outline'}
      class="px-1.5 py-0 text-[10px] font-normal"
    >
      中央: {sync.centralConnected ? '接続' : '切断'}
    </Badge>
    {#if sync.unsyncedEventCount > 0}
      <Badge variant="destructive" class="px-1.5 py-0 text-[10px] font-normal">
        未同期 {sync.unsyncedEventCount}
      </Badge>
    {/if}
  {/if}

  {#if sync.lastSyncedAt !== null}
    <span class="text-muted-foreground text-[10px] truncate ml-auto" title={`最終同期: ${new Date(sync.lastSyncedAt).toLocaleString()}`}>
      同期 {formatRelativeTime(sync.lastSyncedAt)}
    </span>
  {/if}
</div>
