<script lang="ts">
  import { onDestroy } from "svelte";
  import { fade, slide } from "svelte/transition";
  import DockSurface from "./DockSurface.svelte";
  import { dock, type DockContext } from "#lib/stores/dock.svelte.ts";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import { motion } from "#lib/motion.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";

  interface Props {
    ctx: DockContext;
    items: TimelineItem[];
    onclose: () => void;
  }

  let { ctx, items, onclose }: Props = $props();

  /** The flex container that holds the chat and the dock (the handle's parent). */
  let container: HTMLElement | null = null;
  let resizing = $state(false);

  function setWidthFromPointer(event: PointerEvent): void {
    if (!resizing || container === null) return;
    const rect = container.getBoundingClientRect();
    if (rect.width <= 0) return;
    dock.setWidth(Math.round(((rect.right - event.clientX) / rect.width) * 100));
  }

  function stopResize(): void {
    if (!resizing) return;
    resizing = false;
    document.body.style.removeProperty("user-select");
    window.removeEventListener("pointermove", setWidthFromPointer);
    window.removeEventListener("pointerup", stopResize);
    dock.commitWidth();
  }

  function startResize(event: PointerEvent): void {
    if (event.button !== 0) return;
    const parent = (event.currentTarget as HTMLElement).parentElement;
    if (parent === null) return;
    event.preventDefault();
    container = parent;
    resizing = true;
    document.body.style.userSelect = "none";
    window.addEventListener("pointermove", setWidthFromPointer);
    window.addEventListener("pointerup", stopResize);
  }

  function resizeBy(delta: number): void {
    dock.setWidth(dock.width + delta);
    dock.commitWidth();
  }

  onDestroy(stopResize);
</script>

<button
  type="button"
  aria-label={m.dock_resize()}
  in:fade|local={{ duration: motion.duration(120) }}
  out:fade|local={{ duration: motion.duration(120) }}
  class={cn(
    "group z-10 -ml-px flex w-1.5 cursor-col-resize shrink-0 items-center justify-center border-l border-border bg-transparent p-0 outline-none transition-colors select-none hover:bg-signal/50 focus-visible:ring-1 focus-visible:ring-signal active:bg-signal",
    resizing && "bg-signal",
  )}
  onpointerdown={startResize}
  onkeydown={(event) => {
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      resizeBy(2);
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      resizeBy(-2);
    }
  }}
>
  <span
    class="bg-muted-foreground/30 group-hover:bg-signal h-8 w-0.5 rounded-full pointer-events-none"
  ></span>
</button>

<div
  in:slide|local={{ axis: "x", duration: motion.duration(180) }}
  out:slide|local={{ axis: "x", duration: motion.duration(180) }}
  class="flex h-full shrink-0 flex-col overflow-hidden border-l border-border"
  style="width: clamp(22rem, {dock.width}%, 75%);"
>
  <DockSurface {ctx} {items} {onclose} />
</div>
