<script lang="ts">
  import { fade, fly } from "svelte/transition";
  import DockSurface from "./DockSurface.svelte";
  import type { DockContext } from "#lib/stores/dock.svelte.ts";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import { motion } from "#lib/motion.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import XIcon from "@lucide/svelte/icons/x";

  interface Props {
    ctx: DockContext;
    items: TimelineItem[];
    onclose: () => void;
  }

  let { ctx, items, onclose }: Props = $props();
</script>

<!-- The inset panel and the dimmed, blurred backdrop make it obvious that this
     is a layer above the conversation. -->
<div class="absolute inset-0 z-20">
  <button
    type="button"
    tabindex="-1"
    aria-label={m.session_close_panel()}
    transition:fade|local={{ duration: motion.duration(150) }}
    class="bg-background/60 absolute inset-0 cursor-default backdrop-blur-[2px]"
    onclick={onclose}
  ></button>
  <div
    transition:fly|local={{ y: 24, duration: motion.duration(180) }}
    class="bg-card absolute inset-0 flex flex-col overflow-hidden"
  >
    <DockSurface {ctx} {items} {onclose}>
      {#snippet closeIcon()}
        <XIcon class="size-4" />
      {/snippet}
    </DockSurface>
  </div>
</div>
