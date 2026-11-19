import { createEffect, createRoot, flush } from "solid-js";
import { describe, expect, it } from "vitest";
import type { ConnectionStore } from "./connection.ts";
import { SyncStore } from "./sync.ts";

/**
 * `setActiveSession` is driven from the session page's effect, which resets the
 * active session in its cleanup. Reading the state it writes inside that effect
 * would make the effect depend on its own write and re-run forever, and the
 * whole session screen fails to render.
 */
describe("SyncStore active session effect", () => {
  it("can be set from an effect whose cleanup resets it", () => {
    const store = new SyncStore({} as ConnectionStore);
    let runs = 0;
    const dispose = createRoot((disposeRoot) => {
      createEffect(
        // Compute tracks nothing; the write belongs to the apply callback.
        () => 0,
        () => {
          runs += 1;
          if (runs > 5) throw new Error("the effect re-ran from its own write");
          store.setActiveSession("sess-1");
          return () => store.setActiveSession(null);
        },
      );
      return disposeRoot;
    });
    try {
      flush();
      expect(runs).toBe(1);
      expect(store.activeSessionId).toBe("sess-1");
    } finally {
      dispose();
    }
    // Disposing the effect runs its cleanup, which resets the active session.
    expect(store.activeSessionId).toBeNull();
  });
});
