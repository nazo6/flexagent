<script lang="ts">
  import { Popover as PopoverPrimitive } from "bits-ui";
  import { cn } from "#lib/utils.js";
  import PopoverPortal from "./popover-portal.svelte";

  let {
    ref = $bindable(null),
    sideOffset = 6,
    align = "start",
    class: className,
    ...restProps
  }: PopoverPrimitive.ContentProps = $props();
</script>

<PopoverPortal>
  <PopoverPrimitive.Content
    bind:ref
    data-slot="popover-content"
    {sideOffset}
    {align}
    class={cn(
      "bg-popover text-popover-foreground ring-foreground/10 data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 z-50 max-h-(--bits-popover-content-available-height) origin-(--bits-popover-content-transform-origin) overflow-y-auto rounded-lg p-1 shadow-md ring-1 duration-100 outline-none",
      className,
    )}
    {...restProps}
  />
</PopoverPortal>
