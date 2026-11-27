<script lang="ts">
  import { onMount } from "svelte";
  import AppSidebar from "./AppSidebar.svelte";
  import { createEdgeDrawer } from "#lib/edge-drawer.svelte.ts";
  import { registerOverlay } from "#lib/overlays.ts";
  import { m } from "#lib/paraglide/messages.js";
  import XIcon from "@lucide/svelte/icons/x";

  interface Props {
    open: boolean;
    onopen: () => void;
    onclose: () => void;
  }

  let { open, onopen, onclose }: Props = $props();

  let panel = $state<HTMLDivElement | null>(null);

  const drawer = createEdgeDrawer({
    isOpen: () => open,
    width: () => panel?.offsetWidth ?? Math.min(320, window.innerWidth * 0.85),
    panel: () => panel,
    onOpen: () => onopen(),
    onClose: () => onclose(),
  });

  // Settle to the controlled state whenever the finger is not holding the panel.
  $effect(() => {
    drawer.sync();
  });

  // Back closes the drawer like any other overlay.
  $effect(() => {
    if (!open) return;
    return registerOverlay("mobile-drawer", close);
  });

  // Move focus into the drawer, and return it to the trigger on close.
  let previousFocus: HTMLElement | null = null;
  $effect(() => {
    if (open) {
      previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      requestAnimationFrame(() => {
        if (open) panel?.focus({ preventScroll: true });
      });
    } else if (previousFocus !== null) {
      const target = previousFocus;
      previousFocus = null;
      // `inert` is removed from the main area in the same update; wait for it
      // before returning focus to the trigger.
      requestAnimationFrame(() => {
        if (target.isConnected) target.focus({ preventScroll: true });
      });
    }
  });

  /** Stable close callback so the overlay registration does not re-run. */
  function close(): void {
    onclose();
  }

  function onKeydown(event: KeyboardEvent): void {
    if (!open) return;
    if (event.key === "Escape") {
      event.preventDefault();
      close();
      return;
    }
    if (event.key !== "Tab" || panel === null) return;

    const focusables = Array.from(
      panel.querySelectorAll<HTMLElement>(
        'a[href], button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ),
    ).filter((element) => element.offsetParent !== null);
    if (focusables.length === 0) {
      event.preventDefault();
      return;
    }

    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    const active = document.activeElement;
    if (event.shiftKey && (active === first || active === panel)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  }

  onMount(() => drawer.attach());
</script>

<div
  class="fixed inset-0 z-40"
  class:pointer-events-none={drawer.progress === 0 && !drawer.dragging}
  aria-hidden={!open}
  onkeydown={onKeydown}
>
  <button
    type="button"
    tabindex="-1"
    aria-label={m.common_close()}
    class="absolute inset-0 cursor-default bg-black/35"
    class:transition-opacity={!drawer.dragging}
    class:duration-200={!drawer.dragging}
    style={`opacity: ${drawer.progress};`}
    onclick={close}
  ></button>

  <div
    bind:this={panel}
    role="dialog"
    aria-modal={open ? "true" : undefined}
    aria-label={m.shell_navigation()}
    tabindex="-1"
    inert={!open && !drawer.dragging}
    class="safe-area-portal bg-sidebar border-border fixed inset-y-0 left-0 flex w-80 max-w-[85vw] flex-col border-r shadow-2xl outline-none"
    class:transition-transform={!drawer.dragging}
    class:duration-200={!drawer.dragging}
    class:ease-out={!drawer.dragging}
    class:select-none={drawer.dragging}
    style={`transform: translateX(calc(-100% + ${drawer.progress * 100}%)); padding-left: var(--safe-left); touch-action: pan-y;`}
    onclickcapture={drawer.onPanelClick}
  >
    <button
      type="button"
      class="text-muted-foreground hover:bg-accent hover:text-foreground absolute right-2 z-10 flex size-8 items-center justify-center rounded-md"
      style="top: calc(var(--safe-top) + 0.5rem);"
      title={m.common_close()}
      aria-label={m.common_close()}
      onclick={close}
    >
      <XIcon class="size-4" />
    </button>
    <AppSidebar onNavigate={close} />
  </div>
</div>
