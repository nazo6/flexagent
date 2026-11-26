import { browser } from "$app/env";

/**
 * Shared reactive media queries.
 *
 * `max-width: 63.99rem` matches Tailwind's `lg` breakpoint. Below it the app
 * uses its compact shell: drawer navigation and the full-screen dock.
 *
 * `mobileUI` additionally requires a coarse primary pointer. The touch-only
 * surfaces (bottom-sheet pickers) must not replace desktop popovers just
 * because a desktop window was resized narrow; a phone or tablet keeps the
 * mobile interaction model.
 */
class MediaQueryStore {
  matches = $state(false);

  constructor(readonly query: string) {
    if (!browser || typeof window.matchMedia !== "function") return;
    const media = window.matchMedia(query);
    this.matches = media.matches;
    media.addEventListener("change", (event) => {
      this.matches = event.matches;
    });
  }
}

export const mobileLayout = new MediaQueryStore("(max-width: 63.99rem)");
export const mobileUI = new MediaQueryStore("(max-width: 63.99rem) and (pointer: coarse)");
