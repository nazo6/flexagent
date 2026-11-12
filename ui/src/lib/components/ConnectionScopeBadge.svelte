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
  const label = $derived(isHub ? m.shell_scope_hub() : m.shell_scope_node());
  const nodeId = $derived(connection.systemInfo?.node_id ?? "");
  const nodeName = $derived(sync.nodes.find((node) => node.node_id === nodeId)?.name ?? nodeId);

  /** Host shown under the scope label (`hub.example:8080`). */
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
    <span class="flex min-w-0 flex-col leading-tight">
      <span class="text-[10px] font-semibold tracking-wider uppercase">{label}</span>
      <span class="text-muted-foreground truncate text-[11px]">{detail}</span>
    </span>
  </span>
{/if}
