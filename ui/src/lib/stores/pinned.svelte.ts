import { browser } from "$app/environment";

const STORAGE_KEY = "fxg:pinned_sessions";

function loadPinnedIds(): Set<string> {
  if (!browser) return new Set();
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return new Set();
    const parsed = JSON.parse(raw);
    return new Set(Array.isArray(parsed) ? parsed : []);
  } catch {
    return new Set();
  }
}

function savePinnedIds(set: Set<string>): void {
  if (!browser) return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify([...set]));
  } catch {
    /* ignore storage errors */
  }
}

class PinnedStore {
  pinnedIds = $state<Set<string>>(loadPinnedIds());

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
}

export const pinned = new PinnedStore();
