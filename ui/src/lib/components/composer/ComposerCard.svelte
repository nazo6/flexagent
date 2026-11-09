<script lang="ts">
  import { cn } from "#lib/utils.ts";
  import type { Snippet } from "svelte";

  let {
    above = undefined,
    toolbar = undefined,
    class: className = "",
    children,
  }: {
    /** Content above the input (for example workspace pills or notices). */
    above?: Snippet;
    /** Content below the input, separated by a border (selectors and actions). */
    toolbar?: Snippet;
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
  {#if toolbar}
    <div class="flex flex-wrap items-center gap-1.5 border-t px-1 pt-2">
      {@render toolbar()}
    </div>
  {/if}
</div>
