<script lang="ts">
  import ResponsivePicker from "./ResponsivePicker.svelte";
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
    class: className = "",
    contentClass = "",
    open = $bindable(false),
    onopenchange = undefined,
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
    class?: string;
    contentClass?: string;
    open?: boolean;
    /** Called whenever the picker opens or closes. */
    onopenchange?: (open: boolean) => void;
    children?: Snippet;
  } = $props();

  const title = $derived(value === "" ? label : `${label}: ${value}`);
  const triggerClass = $derived(
    cn(
      "border-border bg-chip hover:bg-chip-hover hover:text-foreground focus-visible:ring-ring/40 touch-target motion-press inline-flex h-7 min-w-0 max-w-60 items-center gap-1.5 rounded-full border px-2.5 text-xs max-lg:text-[13px] outline-none focus-visible:ring-2 disabled:pointer-events-none disabled:opacity-50",
      className,
    ),
  );
</script>

{#snippet triggerContent()}
  {@render icon?.()}
  {#if value === ""}
    <span class="text-muted-foreground truncate">{label}</span>
  {:else}
    <span class="truncate">{value}</span>
  {/if}
  {#if loading}
    <LoaderCircleIcon class="text-muted-foreground size-3 shrink-0 animate-spin" />
  {:else}
    <ChevronDownIcon class="text-muted-foreground size-3 shrink-0" />
  {/if}
{/snippet}

<ResponsivePicker bind:open {onopenchange} {label} {title} {triggerClass} {contentClass} {disabled}>
  {#snippet trigger()}
    {@render triggerContent()}
  {/snippet}
  {@render children?.()}
</ResponsivePicker>
