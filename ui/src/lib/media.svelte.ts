import { browser } from "$app/env";

/**
 * Reactive wrapper around `matchMedia`.
 *
 * Instances live in `ui-traits.svelte.ts` so responsive policy has a single
 * source of truth. Read `matches` from a template or `$derived` to subscribe.
 */
export class MediaQueryStore {
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
