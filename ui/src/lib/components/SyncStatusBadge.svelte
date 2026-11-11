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
  const hubLink = $derived(sync.hubLink ?? connection.systemInfo?.hub_link ?? null);
  const hubState = $derived(hubLink?.state ?? "standalone");

  function hubStatusLabel(state: string): string {
    switch (state) {
      case "connected":
        return m.shell_connected();
      case "connecting":
        return m.shell_hub_connecting();
      case "unauthorized":
        return m.shell_hub_unauthorized();
      case "failed":
        return m.shell_hub_unreachable();
      default:
        return m.shell_disconnected();
    }
  }

  function hubStatusVariant(state: string): "secondary" | "outline" | "destructive" {
    if (state === "connected") return "secondary";
    if (state === "standalone") return "outline";
    return "destructive";
  }
</script>

<div class={cn("flex flex-wrap items-center gap-1.5 text-[13px]", className)}>
  <Badge
    variant={sync.wsConnected ? "secondary" : "destructive"}
    class="gap-1 px-1.5 py-0 text-[11px] font-normal"
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
      variant={hubStatusVariant(hubState)}
      class="px-1.5 py-0 text-[11px] font-normal"
      title={hubLink?.message ?? undefined}
    >
      {m.shell_central_status({ status: hubStatusLabel(hubState) })}
    </Badge>
    {#if sync.unsyncedEventCount > 0}
      <Badge variant="destructive" class="px-1.5 py-0 text-[11px] font-normal">
        {m.shell_unsynced_count({ count: sync.unsyncedEventCount })}
      </Badge>
    {/if}
  {/if}

  {#if sync.lastSyncedAt !== null}
    <span
      class="text-muted-foreground text-[11px] ml-auto truncate"
      title={m.shell_last_synced_tooltip({ time: new Date(sync.lastSyncedAt).toLocaleString() })}
    >
      {m.shell_synced_relative({ time: formatRelativeTime(sync.lastSyncedAt) })}
    </span>
  {/if}
</div>
