import { hasOpenOverlay } from "./overlays.ts";
import { DIRECTION_SLOP_PX, EDGE_PX, shouldSettleClosed, shouldSettleOpen } from "./edge-drawer.ts";

/**
 * A touch that starts inside these controls belongs to the control (text
 * selection, cursor placement), not to the drawer.
 */
const PANEL_EXCLUSIONS = "input, textarea, select, [contenteditable='true']";
const EDGE_EXCLUSIONS =
  "a, button, input, textarea, select, label, [role='button'], [contenteditable='true'], [data-no-drawer-swipe]";

/** How long the synthetic click after a drag stays suppressed. */
const CLICK_SUPPRESS_MS = 350;

export interface EdgeDrawerOptions {
  isOpen: () => boolean;
  /** Current drawer width in px, including the live finger position. */
  width: () => number;
  /** The panel element, used to ignore gestures that start in a control. */
  panel: () => HTMLElement | null;
  onOpen: () => void;
  onClose: () => void;
}

/**
 * Touch tracking for an edge drawer.
 *
 * The controller owns the gesture state and the window listeners; the
 * component stays responsible for presentation, focus and the back stack.
 */
export function createEdgeDrawer(options: EdgeDrawerOptions) {
  /** 0 = fully closed, 1 = fully open. Follows the finger while dragging. */
  let progress = $state(0);
  let dragging = $state(false);

  let tracking = false;
  let gesture: "edge" | "panel" | null = null;
  let startX = 0;
  let startY = 0;
  let previousX = 0;
  let lastX = 0;
  let previousAt = 0;
  let lastAt = 0;
  let width = 0;
  let suppressClick = false;
  let suppressTimer: ReturnType<typeof setTimeout> | null = null;

  /** Settle to the controlled state whenever the finger is not holding the panel. */
  function sync(): void {
    if (dragging) return;
    progress = options.isOpen() ? 1 : 0;
  }

  function setProgress(next: number): void {
    progress = Math.max(0, Math.min(1, next));
  }

  function resetGesture(): void {
    tracking = false;
    gesture = null;
  }

  function beginGesture(source: "edge" | "panel", x: number, y: number): void {
    tracking = true;
    gesture = source;
    startX = x;
    startY = y;
    previousX = x;
    lastX = x;
    previousAt = performance.now();
    lastAt = previousAt;
    width = options.width();
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
    }, CLICK_SUPPRESS_MS);
  }

  function finishOpenDrag(): void {
    const open = shouldSettleOpen({
      progress,
      distance: lastX - startX,
      velocity: velocity(),
    });
    if (open) options.onOpen();
    else progress = 0;
    dragging = false;
    resetGesture();
    suppressNextClick();
  }

  function finishCloseDrag(): void {
    const close = shouldSettleClosed({
      progress,
      distance: startX - lastX,
      velocity: velocity(),
    });
    if (close) {
      progress = 0;
      options.onClose();
    } else {
      progress = 1;
    }
    dragging = false;
    resetGesture();
    suppressNextClick();
  }

  /** Whether a touch that started inside the open drawer may drag it closed. */
  function isPanelGesture(event: TouchEvent): boolean {
    const target = event.target;
    const panel = options.panel();
    if (!(target instanceof Node) || panel?.contains(target) !== true) return false;
    return !(target instanceof Element && target.closest(PANEL_EXCLUSIONS));
  }

  function onTouchStart(event: TouchEvent): void {
    if (dragging || tracking || event.touches.length !== 1) return;
    const touch = event.touches[0];

    if (options.isOpen()) {
      if (!isPanelGesture(event)) return;
      beginGesture("panel", touch.clientX, touch.clientY);
      return;
    }

    if (hasOpenOverlay()) return;
    if (touch.clientX > EDGE_PX) return;
    const target = event.target;
    if (target instanceof Element && target.closest(EDGE_EXCLUSIONS)) return;
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

  /** Consume the synthetic click a drag may produce before it activates a row. */
  function onPanelClick(event: MouseEvent): void {
    if (!suppressClick) return;
    suppressClick = false;
    event.preventDefault();
    event.stopPropagation();
  }

  /** Window listeners for the edge gesture; returns the cleanup. */
  function attach(): () => void {
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
  }

  return {
    get progress() {
      return progress;
    },
    get dragging() {
      return dragging;
    },
    sync,
    attach,
    onPanelClick,
  };
}
