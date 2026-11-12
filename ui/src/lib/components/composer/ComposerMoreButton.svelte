<script lang="ts">
  import * as Popover from "#lib/components/ui/popover/index.ts";
  import { cn } from "#lib/utils.ts";
  import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";
  import type { Snippet } from "svelte";

  let {
    label,
    active = false,
    open = $bindable(false),
    contentClass = "",
    children,
  }: {
    /** Accessible name and tooltip of the trigger. */
    label: string;
    /** Show a dot because a hidden option is set. */
    active?: boolean;
    open?: boolean;
    contentClass?: string;
    children?: Snippet;
  } = $props();
</script>

<Popover.Root bind:open>
  <Popover.Trigger
    class={cn(
      "text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-ring/40 relative inline-flex size-7 shrink-0 items-center justify-center rounded-full outline-none transition-colors focus-visible:ring-2",
      active && "text-signal",
    )}
    title={label}
    aria-label={label}
  >
    <MoreHorizontalIcon class="size-4" />
    {#if active}
      <span class="bg-signal absolute top-1 right-1 size-1.5 rounded-full"></span>
    {/if}
  </Popover.Trigger>
  <Popover.Content align="end" class={cn("w-72", contentClass)}>
    {@render children?.()}
  </Popover.Content>
</Popover.Root>
