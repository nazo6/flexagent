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
    "bg-card focus-within:border-primary/40 rounded-2xl border p-2 shadow-sm transition-colors",
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
