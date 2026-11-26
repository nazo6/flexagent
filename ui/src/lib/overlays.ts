/**
 * LIFO stack of overlays that should close before browser history navigates.
 *
 * Android's back gesture and iOS edge swipe both emit a `popstate`. SvelteKit
 * cancels a popstate navigation and restores the previous history entry when
 * `navigation.cancel()` is called, so closing the top overlay here keeps the
 * app on the same route without pushing sentinel history entries.
 *
 * Overlays register while they are open and unregister when their `$effect`
 * cleanup runs (closing or unmounting).
 */
interface OverlayEntry {
  id: string;
  close: () => void;
}

const stack: OverlayEntry[] = [];

/**
 * Register an open overlay. Returns a cleanup function for `$effect`.
 *
 * Registering the same id again replaces the previous entry, so a re-opened
 * overlay always sits on top of the stack.
 */
export function registerOverlay(id: string, close: () => void): () => void {
  if (typeof window === "undefined") return () => {};

  const existing = stack.findIndex((entry) => entry.id === id);
  if (existing !== -1) stack.splice(existing, 1);

  const entry: OverlayEntry = { id, close };
  stack.push(entry);

  return () => {
    const index = stack.indexOf(entry);
    if (index !== -1) stack.splice(index, 1);
  };
}

/**
 * Close the topmost overlay.
 *
 * Returns `true` when an overlay was closed, so the caller can cancel the
 * popstate navigation.
 */
export function closeTopOverlay(): boolean {
  const entry = stack.pop();
  if (entry === undefined) return false;
  entry.close();
  return true;
}

/** Whether any overlay is currently open (for gesture layers). */
export function hasOpenOverlay(): boolean {
  return stack.length > 0;
}
