import { browser } from "$app/env";

/**
 * Shared reactive media queries.
 *
 * `max-width: 63.99rem` matches Tailwind's `lg` breakpoint. Below it the app
 * uses its mobile layout: drawer navigation, the full-screen dock, and bottom
 * sheets instead of anchored popovers.
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
