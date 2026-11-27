<script lang="ts">
  import { cn } from "#lib/utils.ts";
  import CheckIcon from "@lucide/svelte/icons/check";
  import StarIcon from "@lucide/svelte/icons/star";
  import type { Snippet } from "svelte";

  let {
    selected = false,
    disabled = false,
    onselect,
    favorite = null,
    class: className = "",
    children,
  }: {
    selected?: boolean;
    disabled?: boolean;
    onselect?: () => void;
    /**
     * Optional trailing favorite toggle rendered next to (not inside) the row
     * button, so the two controls are never nested.
     */
    favorite?: { active: boolean; label: string; onToggle: () => void } | null;
    class?: string;
    children?: Snippet;
  } = $props();
</script>

<div class="flex w-full items-center gap-1">
  <button
    type="button"
    {disabled}
    class={cn(
      "hover:bg-accent focus-visible:bg-accent flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] transition-colors outline-none disabled:pointer-events-none disabled:opacity-50",
      className,
    )}
    onclick={() => onselect?.()}
  >
    <span class="min-w-0 flex-1">{@render children?.()}</span>
    {#if selected}
      <CheckIcon class="text-signal size-3.5 shrink-0" />
    {/if}
  </button>
  {#if favorite}
    <button
      type="button"
      class={cn(
        "hover:bg-accent flex size-7 shrink-0 items-center justify-center rounded-md transition-colors",
        favorite.active ? "text-signal" : "text-muted-foreground hover:text-foreground",
      )}
      title={favorite.label}
      aria-label={favorite.label}
      aria-pressed={favorite.active}
      onclick={(event) => {
        // Toggling a favorite must not select the row or close the popover.
        event.stopPropagation();
        favorite?.onToggle();
      }}
    >
      <StarIcon class={cn("size-3.5", favorite.active && "fill-current")} />
    </button>
  {/if}
</div>
