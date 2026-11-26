import { MediaQueryStore } from "./media.svelte.ts";

/**
 * Capability traits shared by the responsive layers.
 *
 * Branches are named after what the UI needs, not after a device, so a policy
 * change (a narrow desktop window, a tablet with a trackpad) happens here
 * instead of at every call site.
 *
 * `64rem` is Tailwind's `lg`. `(width < 64rem)` is its exact complement; the
 * old `max-width: 63.99rem` left a 0.01rem gap where neither branch matched.
 */
const compact = new MediaQueryStore("(width < 64rem)");
const coarse = new MediaQueryStore("(pointer: coarse)");
const roomyHeader = new MediaQueryStore("(width >= 48rem)");

class UiTraits {
  /** The viewport is below `lg`; the shell has no room for a fixed sidebar. */
  get compact(): boolean {
    return compact.matches;
  }

  /** The primary input is a finger. */
  get touch(): boolean {
    return coarse.matches;
  }

  /** Navigation is an overlay drawer instead of a persistent sidebar. */
  get overlayNav(): boolean {
    return this.compact;
  }

  /** Workspace tools cover the chat instead of sharing the split pane. */
  get overlayPanels(): boolean {
    return this.compact;
  }

  /** Pickers are bottom sheets instead of anchored popovers. */
  get sheetPickers(): boolean {
    return this.compact && this.touch;
  }

  /** The session header has room for the usage control next to the title. */
  get roomyHeader(): boolean {
    return roomyHeader.matches;
  }
}

export const uiTraits = new UiTraits();
