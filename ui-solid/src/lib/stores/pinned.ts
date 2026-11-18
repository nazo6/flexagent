import { createRoot } from "solid-js";
import { ReactiveCell } from "#lib/stores/reactive.ts";

const STORAGE_KEY = "fxg:pinned_sessions";

function loadPinnedIds(): Set<string> {
  if (typeof localStorage === "undefined") return new Set();
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return new Set();
    const parsed: unknown = JSON.parse(raw);
    return new Set(Array.isArray(parsed) ? parsed : []);
  } catch {
    return new Set();
  }
}

function savePinnedIds(set: Set<string>): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify([...set]));
  } catch {
    /* ignore storage errors */
  }
}

export class PinnedStore {
  readonly #pinnedIds: ReactiveCell<Set<string>>;

  constructor(initial: Set<string> = loadPinnedIds()) {
    this.#pinnedIds = new ReactiveCell(initial);
  }

  get pinnedIds(): Set<string> {
    return this.#pinnedIds.get();
  }

  set pinnedIds(value: Set<string>) {
    this.#pinnedIds.set(value);
  }

  isPinned(sessionId: string): boolean {
    return this.pinnedIds.has(sessionId);
  }

  toggle(sessionId: string): void {
    const next = new Set(this.pinnedIds);
    if (next.has(sessionId)) {
      next.delete(sessionId);
    } else {
      next.add(sessionId);
    }
    this.pinnedIds = next;
    savePinnedIds(next);
  }

  /** Removes every pinned session. */
  clear(): void {
    if (this.pinnedIds.size === 0) return;
    const next = new Set<string>();
    this.pinnedIds = next;
    savePinnedIds(next);
  }
}

export const pinned = createRoot(() => new PinnedStore());
