<script lang="ts">
  import { onMount } from "svelte";
  import AppSidebar from "./AppSidebar.svelte";
  import { hasOpenOverlay, registerOverlay } from "#lib/overlays.ts";
  import { m } from "#lib/paraglide/messages.js";
  import XIcon from "@lucide/svelte/icons/x";

  interface Props {
    open: boolean;
    onopen: () => void;
    onclose: () => void;
  }

  let { open, onopen, onclose }: Props = $props();

  /** Finger travel from the left edge that starts the pull-out gesture. */
  const EDGE_PX = 24;
  /** Movement before we decide between a vertical scroll and a drawer drag. */
  const DIRECTION_SLOP_PX = 6;
  /** Fraction of the drawer width that settles open / closed. */
  const OPEN_THRESHOLD = 0.35;
  const CLOSE_THRESHOLD = 0.65;
  /** Fling speed in px/ms that settles the drawer even before the threshold. */
  const FLING_VELOCITY = 0.45;
  /** A fling must have travelled this far; a tiny quick move is not a fling. */
  const FLING_MIN_DISTANCE_PX = 32;
  const INTERACTIVE_SELECTOR =
    "a, button, input, textarea, select, label, [role='button'], [contenteditable='true'], [data-no-drawer-swipe]";

  let panel = $state<HTMLDivElement | null>(null);
  /** 0 = fully closed, 1 = fully open. Follows the finger while dragging. */
  let progress = $state(0);
  let dragging = $state(false);

  let tracking = false;
  let gesture: "edge" | "panel" | null = null;
  let startX = 0;
  let startY = 0;
  let previousX = 0;
  let lastX = 0;
  let startAt = 0;
  let previousAt = 0;
  let lastAt = 0;
  let width = 0;
  let suppressClick = false;
  let suppressTimer: ReturnType<typeof setTimeout> | null = null;

  // Settle to the controlled state whenever the finger is not holding the panel.
  $effect(() => {
    if (dragging) return;
    progress = open ? 1 : 0;
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

  function panelWidth(): number {
    return panel?.offsetWidth ?? Math.min(320, window.innerWidth * 0.85);
  }

  /** Stable close callback so the overlay registration does not re-run. */
  function close(): void {
    onclose();
  }

  function beginGesture(source: "edge" | "panel", x: number, y: number): void {
    tracking = true;
    gesture = source;
    startX = x;
    startY = y;
    previousX = x;
    lastX = x;
    startAt = performance.now();
    previousAt = startAt;
    lastAt = startAt;
    width = panelWidth();
  }

  function resetGesture(): void {
    tracking = false;
    gesture = null;
  }

  function setProgress(next: number): void {
    progress = Math.max(0, Math.min(1, next));
  }

  /** Speed of the last movement segment, in px/ms. */
  function velocity(): number {
    // Holding still before lifting the finger is not a fling.
    if (performance.now() - lastAt > 100) return 0;
    const elapsed = lastAt - previousAt;
    return elapsed > 0 ? (lastX - previousX) / elapsed : 0;
  }

  /** Keep the synthetic click a drag may produce from activating a row. */
  function suppressNextClick(): void {
    suppressClick = true;
    if (suppressTimer !== null) clearTimeout(suppressTimer);
    suppressTimer = setTimeout(() => {
      suppressClick = false;
      suppressTimer = null;
    }, 350);
  }

  function finishOpenDrag(): void {
    const distance = lastX - startX;
    const shouldOpen =
      progress >= OPEN_THRESHOLD ||
      (distance >= FLING_MIN_DISTANCE_PX && velocity() > FLING_VELOCITY && progress > 0.05);
    if (shouldOpen) onopen();
    else progress = 0;
    dragging = false;
    resetGesture();
    suppressNextClick();
  }

  function finishCloseDrag(): void {
    const distance = startX - lastX;
    const shouldClose =
      progress <= CLOSE_THRESHOLD ||
      (distance >= FLING_MIN_DISTANCE_PX && velocity() < -FLING_VELOCITY && progress < 0.95);
    if (shouldClose) {
      progress = 0;
      close();
    } else {
      progress = 1;
    }
    dragging = false;
    resetGesture();
    suppressNextClick();
  }

  // --- Pull out from the left edge / swipe the open drawer closed ------

  /** Whether a touch that started inside the open drawer may drag it closed. */
  function isPanelGesture(event: TouchEvent): boolean {
    const target = event.target;
    if (!(target instanceof Node) || panel?.contains(target) !== true) return false;
    // A horizontal drag in a text field belongs to the field (selection /
    // cursor), not to the drawer.
    return !(
      target instanceof Element &&
      target.closest("input, textarea, select, [contenteditable='true']")
    );
  }

  function onTouchStart(event: TouchEvent): void {
    if (dragging || tracking || event.touches.length !== 1) return;
    const touch = event.touches[0];

    if (open) {
      if (!isPanelGesture(event)) return;
      beginGesture("panel", touch.clientX, touch.clientY);
      return;
    }

    if (hasOpenOverlay()) return;
    if (touch.clientX > EDGE_PX) return;
    const target = event.target;
    if (target instanceof Element && target.closest(INTERACTIVE_SELECTOR)) return;
    beginGesture("edge", touch.clientX, touch.clientY);
  }

  function onTouchMove(event: TouchEvent): void {
    if (!tracking || event.touches.length !== 1) return;
    const touch = event.touches[0];
    const dx = touch.clientX - startX;
    const dy = touch.clientY - startY;

    if (!dragging) {
      // Vertical movement belongs to the page / list, not the drawer.
      if (Math.abs(dy) > Math.abs(dx) && Math.abs(dy) > DIRECTION_SLOP_PX) {
        resetGesture();
        return;
      }
      const horizontal =
        gesture === "edge"
          ? dx >= DIRECTION_SLOP_PX && dx > Math.abs(dy)
          : dx <= -DIRECTION_SLOP_PX && Math.abs(dx) > Math.abs(dy);
      if (!horizontal) return;
      dragging = true;
    }

    // Horizontal now: stop the browser from scrolling the page/list while the
    // drawer follows the finger.
    event.preventDefault();
    previousX = lastX;
    previousAt = lastAt;
    lastX = touch.clientX;
    lastAt = performance.now();
    setProgress(gesture === "edge" ? dx / width : 1 + dx / width);
  }

  function onTouchEnd(): void {
    if (!tracking) return;
    if (dragging && gesture === "edge") finishOpenDrag();
    else if (dragging && gesture === "panel") finishCloseDrag();
    else resetGesture();
  }

  function onPanelClick(event: MouseEvent): void {
    if (!suppressClick) return;
    suppressClick = false;
    event.preventDefault();
    event.stopPropagation();
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

  onMount(() => {
    window.addEventListener("touchstart", onTouchStart, { passive: true });
    window.addEventListener("touchmove", onTouchMove, { passive: false });
    window.addEventListener("touchend", onTouchEnd);
    window.addEventListener("touchcancel", onTouchEnd);
    return () => {
      window.removeEventListener("touchstart", onTouchStart);
      window.removeEventListener("touchmove", onTouchMove);
      window.removeEventListener("touchend", onTouchEnd);
      window.removeEventListener("touchcancel", onTouchEnd);
      if (suppressTimer !== null) clearTimeout(suppressTimer);
    };
  });
</script>

<div
  class="fixed inset-0 z-40"
  class:pointer-events-none={progress === 0 && !dragging}
  aria-hidden={!open}
  onkeydown={onKeydown}
>
  <button
    type="button"
    tabindex="-1"
    aria-label={m.common_close()}
    class="absolute inset-0 cursor-default bg-black/35"
    class:transition-opacity={!dragging}
    class:duration-200={!dragging}
    style={`opacity: ${progress};`}
    onclick={close}
  ></button>

  <div
    bind:this={panel}
    role="dialog"
    aria-modal={open ? "true" : undefined}
    aria-label={m.shell_navigation()}
    tabindex="-1"
    inert={!open && !dragging}
    class="safe-area-portal bg-sidebar border-border fixed inset-y-0 left-0 flex w-80 max-w-[85vw] flex-col border-r shadow-2xl outline-none"
    class:transition-transform={!dragging}
    class:duration-200={!dragging}
    class:ease-out={!dragging}
    class:select-none={dragging}
    style={`transform: translateX(calc(-100% + ${progress * 100}%)); padding-left: var(--safe-left); touch-action: pan-y;`}
    onclickcapture={onPanelClick}
  >
    <button
      type="button"
      class="text-muted-foreground hover:bg-accent hover:text-foreground touch-target absolute right-2 z-10 flex items-center justify-center rounded-md"
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
