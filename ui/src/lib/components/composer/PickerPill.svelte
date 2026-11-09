<script lang="ts">
  import * as Popover from "#lib/components/ui/popover/index.ts";
  import { cn } from "#lib/utils.ts";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import type { Snippet } from "svelte";

  let {
    label,
    value = "",
    icon = undefined,
    disabled = false,
    loading = false,
    showLabel = false,
    class: className = "",
    contentClass = "",
    open = $bindable(false),
    children,
  }: {
    /** Accessible name (for example "Project" or "Model"). */
    label: string;
    /** Current value shown in the trigger. */
    value?: string;
    /** Optional leading icon snippet. */
    icon?: Snippet;
    disabled?: boolean;
    loading?: boolean;
    /** Show the label before the value (useful for less self-explanatory options). */
    showLabel?: boolean;
    class?: string;
    contentClass?: string;
    open?: boolean;
    children?: Snippet;
  } = $props();
</script>

<Popover.Root bind:open>
  <Popover.Trigger
    {disabled}
    class={cn(
      "border-border bg-chip hover:bg-chip-hover hover:text-foreground focus-visible:ring-ring/40 inline-flex h-7 min-w-0 max-w-60 items-center gap-1.5 rounded-full border px-2.5 text-xs transition-colors outline-none focus-visible:ring-2 disabled:pointer-events-none disabled:opacity-50",
      className,
    )}
    title={value === "" ? label : `${label}: ${value}`}
    aria-label={value === "" ? label : `${label}: ${value}`}
  >
    {@render icon?.()}
    {#if value === ""}
      <span class="text-muted-foreground truncate">{label}</span>
    {:else}
      {#if showLabel}
        <span class="text-muted-foreground shrink-0">{label}</span>
      {/if}
      <span class="truncate">{value}</span>
    {/if}
    {#if loading}
      <LoaderCircleIcon class="text-muted-foreground size-3 shrink-0 animate-spin" />
    {:else}
      <ChevronDownIcon class="text-muted-foreground size-3 shrink-0" />
    {/if}
  </Popover.Trigger>
  <Popover.Content class={cn("w-72", contentClass)}>
    {@render children?.()}
  </Popover.Content>
</Popover.Root>
