/**
 * True when the primary input is a coarse pointer such as a finger.
 *
 * Picker popovers focus their first tabbable element when they open. On a touch
 * device that would pop up the on-screen keyboard for the search field, so
 * callers skip the auto-focus there while keeping type-to-filter on desktop.
 */
export function isCoarsePointer(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false;
  return window.matchMedia("(pointer: coarse)").matches;
}
