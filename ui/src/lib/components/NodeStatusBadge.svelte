<script lang="ts">
  import { getNodeAvailability } from "#lib/node-status.ts";
  import type { NodeSummary } from "#lib/generated/bindings.ts";
  import { cn } from "#lib/utils.ts";

  interface Props {
    node?: NodeSummary | null;
    showLabel?: boolean;
    showNodeName?: boolean;
    class?: string;
  }

  let {
    node = null,
    showLabel = true,
    showNodeName = false,
    class: className = "",
  }: Props = $props();

  const availability = $derived(getNodeAvailability(node));
</script>

<div
  class={cn("inline-flex items-center gap-1.5 text-2xs", className)}
  title={node
    ? `${node.name} (${node.os}/${node.arch}): ${availability.statusText}`
    : availability.statusText}
>
  <span class={cn("size-2 rounded-full shrink-0", availability.dotColorClass)}></span>
  {#if showNodeName && node}
    <span class="font-medium text-foreground">{node.name}</span>
  {/if}
  {#if showLabel}
    <span class="text-muted-foreground">{availability.statusText}</span>
  {/if}
</div>
