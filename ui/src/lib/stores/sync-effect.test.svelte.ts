import { flushSync } from "svelte";
import { describe, expect, it } from "vitest";
import type { ConnectionStore } from "./connection.svelte.ts";
import { SyncStore } from "./sync.svelte.ts";

/**
 * `setActiveSession` is driven from the session page's effect, which resets the
 * active session in its cleanup. Reading the state it writes inside that effect
 * makes Svelte re-run it forever (`effect_update_depth_exceeded`), and the whole
 * session screen fails to render.
 */
describe("SyncStore active session effect", () => {
  it("can be set from an effect whose cleanup resets it", () => {
    const store = new SyncStore({} as ConnectionStore);
    const stop = $effect.root(() => {
      $effect(() => {
        store.setActiveSession("sess-1");
        return () => store.setActiveSession(null);
      });
    });
    try {
      expect(() => flushSync()).not.toThrow();
      expect(store.activeSessionId).toBe("sess-1");
    } finally {
      stop();
    }
  });
});
