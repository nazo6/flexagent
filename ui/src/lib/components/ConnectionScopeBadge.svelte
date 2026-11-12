<script lang="ts">
  import { connection, sync } from "#lib/stores/app.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import { m } from "#lib/paraglide/messages.js";
  import CpuIcon from "@lucide/svelte/icons/cpu";
  import ServerIcon from "@lucide/svelte/icons/server";

  interface Props {
    /** Small single-line chip for tight headers (mobile). */
    compact?: boolean;
    class?: string;
  }

  let { compact = false, class: className }: Props = $props();

  const isHub = $derived(connection.role === "hub");
  const isLocalNode = $derived(connection.role === "local_node");
  const label = $derived(isHub ? m.shell_scope_hub() : m.shell_scope_node());
  const nodeId = $derived(connection.systemInfo?.node_id ?? "");
  const nodeName = $derived(sync.nodes.find((node) => node.node_id === nodeId)?.name ?? nodeId);

  /** Host shown next to the scope label (`hub.example:8080`). */
  const host = $derived.by(() => {
    const origin = connection.origin;
    try {
      return origin === "" ? location.host : new URL(origin).host;
    } catch {
      return origin;
    }
  });

  const detail = $derived(isHub ? host : nodeName || host);
  const title = $derived(
    isHub
      ? m.shell_scope_hub_tooltip({ target: host })
      : m.shell_scope_node_tooltip({ target: detail }),
  );
  const icon = $derived(isHub ? ServerIcon : CpuIcon);

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

  /** Link health of this client and, on a node, of the node's hub link. */
  const healthText = $derived.by(() => {
    const parts = [sync.wsConnected ? m.shell_connected() : m.shell_disconnected()];
    if (isLocalNode) {
      parts.push(m.shell_hub_status({ status: hubStatusLabel(hubState) }));
    }
    if (sync.unsyncedEventCount > 0) {
      parts.push(m.shell_unsynced_count({ count: sync.unsyncedEventCount }));
    }
    return parts.join(" · ");
  });

  const healthDotClass = $derived(
    !sync.wsConnected
      ? "bg-danger"
      : isLocalNode && hubState !== "standalone" && hubState !== "connected"
        ? "bg-warn"
        : "bg-ok",
  );
</script>

{#if compact}
  <span
    class={cn(
      "inline-flex shrink-0 items-center gap-1 rounded-md border px-1.5 py-0.5 text-[10px] font-semibold tracking-wider uppercase",
      isHub ? "border-signal/50 bg-signal-soft text-foreground" : "border-border bg-muted/60",
      className,
    )}
    {title}
  >
    {#if isHub}
      <ServerIcon class="size-3 shrink-0" />
    {:else}
      <CpuIcon class="size-3 shrink-0" />
    {/if}
    {label}
  </span>
{:else}
  {@const ScopeIcon = icon}
  <span
    class={cn(
      "flex min-w-0 items-center gap-2 rounded-md border px-2 py-1.5",
      isHub ? "border-signal/50 bg-signal-soft/60" : "border-border bg-muted/40",
      className,
    )}
    {title}
  >
    <ScopeIcon class="text-muted-foreground size-3.5 shrink-0" />
    <span class="flex min-w-0 flex-col gap-0.5 leading-tight">
      <span class="flex min-w-0 items-baseline gap-1.5">
        <span class="shrink-0 text-[10px] font-semibold tracking-wider uppercase">{label}</span>
        <span class="text-muted-foreground truncate text-[11px]">{detail}</span>
      </span>
      <span class="text-muted-foreground flex min-w-0 items-center gap-1.5 text-[11px]">
        <span class={cn("size-1.5 shrink-0 rounded-full", healthDotClass)}></span>
        <span class="truncate">{healthText}</span>
      </span>
    </span>
  </span>
{/if}
