<script lang="ts">
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import { connection, sync } from "#lib/stores/app.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import { m } from "#lib/paraglide/messages.js";
  import WifiIcon from "@lucide/svelte/icons/wifi";
  import WifiOffIcon from "@lucide/svelte/icons/wifi-off";

  interface Props {
    class?: string;
  }

  let { class: className }: Props = $props();

  const isLocalNode = $derived(connection.role === "local_node");
</script>

<div class={cn("flex flex-wrap items-center gap-1.5 text-xs", className)}>
  <Badge
    variant={sync.wsConnected ? "secondary" : "destructive"}
    class="gap-1 px-1.5 py-0 text-[10px] font-normal"
  >
    {#if sync.wsConnected}
      <WifiIcon class="size-3 shrink-0" />
      <span>{m.shell_connected()}</span>
    {:else}
      <WifiOffIcon class="size-3 shrink-0" />
      <span>{m.shell_disconnected()}</span>
    {/if}
  </Badge>

  {#if isLocalNode}
    <Badge
      variant={sync.centralConnected ? "secondary" : "outline"}
      class="px-1.5 py-0 text-[10px] font-normal"
    >
      {m.shell_central_status({
        status: sync.centralConnected ? m.shell_connected() : m.shell_disconnected(),
      })}
    </Badge>
    {#if sync.unsyncedEventCount > 0}
      <Badge variant="destructive" class="px-1.5 py-0 text-[10px] font-normal">
        {m.shell_unsynced_count({ count: sync.unsyncedEventCount })}
      </Badge>
    {/if}
  {/if}

  {#if sync.lastSyncedAt !== null}
    <span
      class="text-muted-foreground text-[10px] truncate ml-auto"
      title={m.shell_last_synced_tooltip({ time: new Date(sync.lastSyncedAt).toLocaleString() })}
    >
      {m.shell_synced_relative({ time: formatRelativeTime(sync.lastSyncedAt) })}
    </span>
  {/if}
</div>
