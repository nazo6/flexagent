<script lang="ts">
  import { cn } from "#lib/utils.ts";
  import CheckIcon from "@lucide/svelte/icons/check";
  import type { Snippet } from "svelte";

  let {
    selected = false,
    disabled = false,
    onselect,
    class: className = "",
    children,
  }: {
    selected?: boolean;
    disabled?: boolean;
    onselect?: () => void;
    class?: string;
    children?: Snippet;
  } = $props();
</script>

<button
  type="button"
  {disabled}
  class={cn(
    "hover:bg-accent focus-visible:bg-accent flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] transition-colors outline-none disabled:pointer-events-none disabled:opacity-50",
    className,
  )}
  onclick={() => onselect?.()}
>
  <span class="min-w-0 flex-1">{@render children?.()}</span>
  {#if selected}
    <CheckIcon class="text-signal size-3.5 shrink-0" />
  {/if}
</button>
