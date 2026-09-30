<script lang="ts">
  import { Badge } from '$lib/components/ui/badge';
  import { formatRelativeTime } from '$lib/format';
  import { connection, sync } from '$lib/stores/app.svelte';
  import WifiIcon from '@lucide/svelte/icons/wifi';
  import WifiOffIcon from '@lucide/svelte/icons/wifi-off';

  const isLocalNode = $derived(connection.systemInfo?.role === 'local_node');
</script>

<div class="flex items-center gap-1.5">
  <Badge variant={sync.wsConnected ? 'secondary' : 'destructive'}>
    {#if sync.wsConnected}
      <WifiIcon />
      接続中
    {:else}
      <WifiOffIcon />
      切断
    {/if}
  </Badge>

  {#if isLocalNode}
    <Badge variant={sync.centralConnected ? 'secondary' : 'outline'}>
      中央: {sync.centralConnected ? '接続' : '切断'}
    </Badge>
    {#if sync.unsyncedEventCount > 0}
      <Badge variant="destructive">未同期 {sync.unsyncedEventCount}</Badge>
    {/if}
  {/if}

  {#if sync.lastSyncedAt !== null}
    <span class="text-muted-foreground hidden text-xs lg:inline">
      最終同期 {formatRelativeTime(sync.lastSyncedAt)}
    </span>
  {/if}
</div>
