<script lang="ts">
  import { cn } from "#lib/utils.ts";
  import type { Snippet } from "svelte";

  let {
    above = undefined,
    selectors = undefined,
    actions = undefined,
    class: className = "",
    children,
  }: {
    /** Content above the input (for example workspace pills or notices). */
    above?: Snippet;
    /**
     * Selectors below the input (agent / mode / model / other options).
     *
     * The row scrolls horizontally instead of wrapping, so the action cluster
     * on the right never moves.
     */
    selectors?: Snippet;
    /** Actions below the input, pinned to the right and never wrapped. */
    actions?: Snippet;
    class?: string;
    /** The input area (usually a `ComposerTextarea`). */
    children?: Snippet;
  } = $props();
</script>

<div
  class={cn(
    "bg-card focus-within:border-signal/50 focus-within:ring-signal/15 rounded-lg border border-border p-2.5 transition-[border-color,box-shadow] focus-within:ring-2",
    className,
  )}
>
  {#if above}
    {@render above()}
  {/if}
  {@render children?.()}
  {#if selectors || actions}
    <div class="border-border flex items-center gap-1.5 border-t px-0.5 pt-1.5">
      <div class="relative min-w-0 flex-1">
        <div
          class="flex items-center gap-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        >
          {@render selectors?.()}
        </div>
        <div
          class="from-card pointer-events-none absolute inset-y-0 right-0 w-8 bg-gradient-to-l to-transparent"
        ></div>
      </div>
      <div class="flex shrink-0 items-center gap-1">
        {@render actions?.()}
      </div>
    </div>
  {/if}
</div>
