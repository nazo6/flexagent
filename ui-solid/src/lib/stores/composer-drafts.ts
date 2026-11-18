import type { AttachmentMeta } from "#lib/generated/bindings.ts";

/**
 * Per-composer drafts.
 *
 * The prompt text is remembered on this device per session, so navigating to
 * another screen (or reloading) does not discard it. Attachments stay in memory
 * only: inline images can be several megabytes and would exceed the
 * localStorage quota.
 */

const DRAFT_KEY_PREFIX = "fxg:composer_draft:";

/** Draft key for the new-session screen (not tied to a session ID). */
export const NEW_SESSION_DRAFT_ID = "new-session";

/** Resolve `localStorage` (SSR / tests can inject a replacement). */
function resolveStorage(storage?: Storage | null): Storage | null {
  if (storage !== undefined) return storage;
  return typeof localStorage === "undefined" ? null : localStorage;
}

/** Load the saved prompt text for one composer. */
export function loadComposerDraft(sessionId: string, storage?: Storage | null): string {
  const store = resolveStorage(storage);
  if (store === null || sessionId === "") return "";
  try {
    return store.getItem(DRAFT_KEY_PREFIX + sessionId) ?? "";
  } catch {
    return "";
  }
}

/** Save the prompt text for one composer (empty text removes the stored draft). */
export function saveComposerDraft(sessionId: string, text: string, storage?: Storage | null): void {
  const store = resolveStorage(storage);
  if (store === null || sessionId === "") return;
  try {
    if (text === "") {
      store.removeItem(DRAFT_KEY_PREFIX + sessionId);
    } else {
      store.setItem(DRAFT_KEY_PREFIX + sessionId, text);
    }
  } catch {
    // Do not persist when localStorage is disabled or over quota
  }
}

interface ComposerDraft {
  text: string;
  attachments: AttachmentMeta[];
}

/**
 * Drafts keyed by session ID (plus the new-session screen).
 *
 * Components keep their own `$state` for editing and mirror changes here. The
 * store outlives route components, so a draft survives navigation.
 */
export class ComposerDraftStore {
  #drafts = new Map<string, ComposerDraft>();
  #storage: Storage | null | undefined;

  constructor(storage?: Storage | null) {
    this.#storage = storage;
  }

  /** Draft for one composer, loaded from this device on first use. */
  get(sessionId: string): ComposerDraft {
    const existing = this.#drafts.get(sessionId);
    if (existing !== undefined) return existing;
    const created: ComposerDraft = {
      text: loadComposerDraft(sessionId, this.#storage),
      attachments: [],
    };
    this.#drafts.set(sessionId, created);
    return created;
  }

  /** Update the prompt text (persisted to this device). */
  setText(sessionId: string, text: string): void {
    const draft = this.get(sessionId);
    if (draft.text === text) return;
    draft.text = text;
    saveComposerDraft(sessionId, text, this.#storage);
    this.#prune(sessionId);
  }

  /** Update the attachments (in-memory only). */
  setAttachments(sessionId: string, attachments: AttachmentMeta[]): void {
    const draft = this.get(sessionId);
    if (
      draft.attachments.length === attachments.length &&
      draft.attachments.every((attachment, index) => attachment === attachments[index])
    ) {
      return;
    }
    draft.attachments = [...attachments];
    this.#prune(sessionId);
  }

  /** Drop all state and the stored text for one composer. */
  clear(sessionId: string): void {
    this.#drafts.delete(sessionId);
    saveComposerDraft(sessionId, "", this.#storage);
  }

  #prune(sessionId: string): void {
    const draft = this.#drafts.get(sessionId);
    if (draft !== undefined && draft.text === "" && draft.attachments.length === 0) {
      this.#drafts.delete(sessionId);
    }
  }
}

export const composerDrafts = new ComposerDraftStore();
