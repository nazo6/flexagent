import { ApiError } from "#lib/api/errors.ts";
import type { ApiClient } from "#lib/api/client.ts";
import type { AttachmentMeta, Draft } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { randomUuid } from "#lib/uuid.ts";
import { SvelteMap } from "svelte/reactivity";
import { toast } from "svelte-sonner";

/**
 * Multiple composer drafts, persisted by the connected endpoint (hub / node).
 *
 * The server is the source of truth. This store keeps a per-scope cache for
 * synchronous rendering and debounces autosaves. Selection and offline edits
 * stay device-local; the body and title are shared. See
 * `docs/proposals/2026-11-27-composer-drafts-and-sync.md`.
 */

/** Delay before mirroring an edit to the server (coalesces keystrokes). */
const SAVE_DEBOUNCE_MS = 800;
/** Legacy single-draft key (migrated into the first server draft). */
const LEGACY_KEY_PREFIX = "fxg:composer_draft:";
/** Per-scope selected draft ID (device-local). */
const ACTIVE_KEY_PREFIX = "fxg:composer_draft_active:";
/** Device ID used for conflict diagnostics (device-local). */
const DEVICE_ID_KEY = "fxg:device_id";

/** Scope key for the new-session screen (not tied to a session ID). */
export const NEW_SESSION_SCOPE = "new";

/** Storage key for one composer scope. */
export function draftScope(sessionId: string | null): string {
  return sessionId === null || sessionId === "" ? NEW_SESSION_SCOPE : `session:${sessionId}`;
}

/** Title shown in the draft picker (explicit title, else the first non-empty line). */
export function draftTitle(draft: Pick<Draft, "title" | "body">): string {
  const title = draft.title.trim();
  if (title !== "") return title;
  const first = draft.body
    .split("\n")
    .map((line) => line.trim())
    .find((line) => line !== "");
  return first === undefined ? m.drafts_untitled() : first.slice(0, 80);
}

function resolveStorage(storage?: Storage | null): Storage | null {
  if (storage !== undefined) return storage;
  return typeof localStorage === "undefined" ? null : localStorage;
}

/** Stable per-browser device ID (used only for conflict diagnostics). */
export function loadDeviceId(storage?: Storage | null): string {
  const store = resolveStorage(storage);
  if (store !== null) {
    try {
      const existing = store.getItem(DEVICE_ID_KEY);
      if (existing !== null && existing !== "") return existing;
    } catch {
      // Fall through to a fresh in-memory ID.
    }
  }
  const generated = randomUuid();
  if (store !== null) {
    try {
      store.setItem(DEVICE_ID_KEY, generated);
    } catch {
      // Storage may be disabled or full; the ID is best effort.
    }
  }
  return generated;
}

/** Selected draft for one scope (device-local). */
export function loadActiveDraftId(scope: string, storage?: Storage | null): string | null {
  const store = resolveStorage(storage);
  if (store === null) return null;
  try {
    return store.getItem(ACTIVE_KEY_PREFIX + scope);
  } catch {
    return null;
  }
}

/** Persists the selected draft for one scope (device-local). */
export function saveActiveDraftId(scope: string, draftId: string, storage?: Storage | null): void {
  const store = resolveStorage(storage);
  if (store === null) return;
  try {
    store.setItem(ACTIVE_KEY_PREFIX + scope, draftId);
  } catch {
    // Selection is a convenience; ignore quota / disabled storage.
  }
}

function loadLegacyDraft(sessionId: string, storage?: Storage | null): string {
  const store = resolveStorage(storage);
  if (store === null) return "";
  try {
    return store.getItem(LEGACY_KEY_PREFIX + sessionId) ?? "";
  } catch {
    return "";
  }
}

function clearLegacyDraft(sessionId: string, storage?: Storage | null): void {
  const store = resolveStorage(storage);
  if (store === null) return;
  try {
    store.removeItem(LEGACY_KEY_PREFIX + sessionId);
  } catch {
    // Ignore quota / disabled storage.
  }
}

/** Optional context carried by a session-less draft. */
export interface DraftContext {
  projectId?: string | null;
  nodeId?: string | null;
}

/** Draft autosave status for the composer indicator. */
export type DraftSaveStatus = "saving" | "saved" | "error";

/**
 * Server-backed draft cache and autosave controller.
 *
 * Lists are keyed by composer scope (`session:<id>` / `new`). Values are
 * replaced (not mutated) so SvelteMap subscribers re-render.
 */
export class ComposerDraftStore {
  #drafts = new SvelteMap<string, Draft[]>();
  #active = new SvelteMap<string, string>();
  #status = new SvelteMap<string, DraftSaveStatus>();
  #apis = new Map<string, ApiClient>();
  #loaded = new Set<string>();
  #timers = new Map<string, ReturnType<typeof setTimeout>>();
  #deviceId: string;
  #storage: Storage | null | undefined;

  constructor(storage?: Storage | null) {
    this.#storage = storage;
    this.#deviceId = loadDeviceId(storage);
  }

  /** Current drafts for one scope (reactive when read inside `$derived`). */
  list(scope: string): Draft[] {
    return this.#drafts.get(scope) ?? [];
  }

  /** Autosave status of one draft (`null` before the first save attempt). */
  saveStatus(draftId: string): DraftSaveStatus | null {
    return this.#status.get(draftId) ?? null;
  }

  /** The selected draft, or `null` while the first load is in flight. */
  activeDraft(scope: string): Draft | null {
    const activeId = this.#active.get(scope);
    if (activeId === undefined) return null;
    return this.list(scope).find((draft) => draft.draft_id === activeId) ?? null;
  }

  /**
   * Loads the scope once. A scope with no drafts gets one (migrating the old
   * localStorage draft on the first run) so the composer always has an editor.
   */
  async ensure(
    scope: string,
    sessionId: string | null,
    api: ApiClient,
    context: DraftContext = {},
  ): Promise<Draft | null> {
    this.#apis.set(scope, api);
    if (this.#loaded.has(scope)) return this.activeDraft(scope);
    this.#loaded.add(scope);

    try {
      let drafts = await this.#fetch(api, sessionId);
      if (drafts.length === 0) {
        // The pre-multi-draft store used `fxg:composer_draft:<id>`; the
        // new-session screen used the fixed id "new-session".
        const legacyId = sessionId ?? "new-session";
        const legacy = loadLegacyDraft(legacyId, this.#storage);
        drafts = [await this.#createRemote(api, sessionId, legacy, context)];
        if (legacy !== "") clearLegacyDraft(legacyId, this.#storage);
      }
      this.#drafts.set(scope, drafts);
      this.#restoreActive(scope, drafts);
      return this.activeDraft(scope);
    } catch (error) {
      this.#loaded.delete(scope);
      throw error;
    }
  }

  /** Creates a new empty draft and selects it. */
  async create(
    scope: string,
    sessionId: string | null,
    api: ApiClient,
    context: DraftContext = {},
  ): Promise<Draft> {
    this.#apis.set(scope, api);
    const draft = await this.#createRemote(api, sessionId, "", context);
    this.#drafts.set(scope, [...this.list(scope), draft]);
    this.select(scope, draft.draft_id);
    return draft;
  }

  /** Selects a draft in one scope (device-local). */
  select(scope: string, draftId: string): void {
    if (!this.list(scope).some((draft) => draft.draft_id === draftId)) return;
    this.#active.set(scope, draftId);
    saveActiveDraftId(scope, draftId, this.#storage);
  }

  /** Updates the body locally and schedules a debounced save. */
  setText(scope: string, draftId: string, body: string): void {
    const drafts = this.list(scope);
    const index = drafts.findIndex((draft) => draft.draft_id === draftId);
    if (index < 0 || drafts[index].body === body) return;
    const next = drafts.slice();
    next[index] = { ...next[index], body };
    this.#drafts.set(scope, next);
    this.#schedule(scope, draftId);
  }

  /** Updates the display title locally and schedules a debounced save. */
  rename(scope: string, draftId: string, title: string): void {
    const drafts = this.list(scope);
    const index = drafts.findIndex((draft) => draft.draft_id === draftId);
    if (index < 0 || drafts[index].title === title) return;
    const next = drafts.slice();
    next[index] = { ...next[index], title };
    this.#drafts.set(scope, next);
    this.#schedule(scope, draftId);
  }

  /** Updates the attachments locally (v1 does not persist attachment payloads). */
  setAttachments(scope: string, draftId: string, attachments: AttachmentMeta[]): void {
    const drafts = this.list(scope);
    const index = drafts.findIndex((draft) => draft.draft_id === draftId);
    if (index < 0) return;
    const next = drafts.slice();
    next[index] = { ...next[index], attachments: [...attachments] };
    this.#drafts.set(scope, next);
  }

  /** Deletes one draft and selects the next one (creates a draft when empty). */
  async remove(
    scope: string,
    draftId: string,
    sessionId: string | null,
    api: ApiClient,
  ): Promise<void> {
    this.#cancelSave(draftId);
    this.#status.delete(draftId);
    await this.#deleteRemote(api, draftId);
    const remaining = this.list(scope).filter((draft) => draft.draft_id !== draftId);
    this.#drafts.set(scope, remaining);
    const next = remaining[0];
    if (next !== undefined) {
      this.select(scope, next.draft_id);
    } else {
      await this.create(scope, sessionId, api);
    }
  }

  /** Deletes one draft without selecting or creating a replacement. */
  async discard(scope: string, draftId: string, api: ApiClient): Promise<void> {
    this.#cancelSave(draftId);
    this.#status.delete(draftId);
    await this.#deleteRemote(api, draftId);
    this.#drafts.set(
      scope,
      this.list(scope).filter((draft) => draft.draft_id !== draftId),
    );
  }

  /** Drops a sent draft and prepares a fresh one (session composers). */
  async consume(
    scope: string,
    draftId: string,
    sessionId: string | null,
    api: ApiClient,
  ): Promise<void> {
    await this.remove(scope, draftId, sessionId, api);
  }

  /** Forgets a scope (the new-session screen after starting a session). */
  clearScope(scope: string): void {
    for (const draft of this.list(scope)) {
      this.#cancelSave(draft.draft_id);
      this.#status.delete(draft.draft_id);
    }
    this.#drafts.delete(scope);
    this.#active.delete(scope);
    this.#apis.delete(scope);
    this.#loaded.delete(scope);
  }

  /** Flushes a pending autosave immediately (e.g. before navigation). */
  async flush(scope: string, draftId: string): Promise<void> {
    const timer = this.#timers.get(draftId);
    if (timer !== undefined) {
      clearTimeout(timer);
      this.#timers.delete(draftId);
    }
    await this.#save(scope, draftId);
  }

  /** Retries a failed autosave immediately. */
  async retry(scope: string, draftId: string): Promise<void> {
    this.#cancelSave(draftId);
    await this.#save(scope, draftId);
  }

  async #fetch(api: ApiClient, sessionId: string | null): Promise<Draft[]> {
    if (sessionId === null) return api.drafts({ sessionless: true });
    return api.drafts({ sessionId });
  }

  #createRemote(
    api: ApiClient,
    sessionId: string | null,
    body: string,
    context: DraftContext,
  ): Promise<Draft> {
    return api.createDraft({
      title: "",
      body,
      session_id: sessionId,
      project_id: context.projectId ?? null,
      node_id: context.nodeId ?? null,
      attachments: [],
      origin_device_id: this.#deviceId,
    });
  }

  #restoreActive(scope: string, drafts: Draft[]): void {
    const saved = loadActiveDraftId(scope, this.#storage);
    const active = drafts.find((draft) => draft.draft_id === saved) ?? drafts[0];
    if (active !== undefined) {
      this.#active.set(scope, active.draft_id);
      saveActiveDraftId(scope, active.draft_id, this.#storage);
    }
  }

  #schedule(scope: string, draftId: string): void {
    const existing = this.#timers.get(draftId);
    if (existing !== undefined) clearTimeout(existing);
    this.#timers.set(
      draftId,
      setTimeout(() => {
        this.#timers.delete(draftId);
        void this.#save(scope, draftId);
      }, SAVE_DEBOUNCE_MS),
    );
  }

  #cancelSave(draftId: string): void {
    const timer = this.#timers.get(draftId);
    if (timer !== undefined) {
      clearTimeout(timer);
      this.#timers.delete(draftId);
    }
  }

  async #save(scope: string, draftId: string): Promise<void> {
    const api = this.#apis.get(scope);
    if (api === undefined) return;
    const draft = this.list(scope).find((item) => item.draft_id === draftId);
    if (draft === undefined) return;

    this.#status.set(draftId, "saving");
    try {
      const updated = await api.updateDraft(draftId, {
        expected_revision: draft.revision,
        title: draft.title,
        body: draft.body,
        session_id: draft.session_id,
        project_id: draft.project_id,
        node_id: draft.node_id,
        // Attachment payloads stay on the device; do not persist megabytes in SQLite.
        attachments: [],
        origin_device_id: this.#deviceId,
      });
      this.#replace(scope, updated);
      this.#status.set(draftId, "saved");
    } catch (error) {
      if (error instanceof ApiError && error.code === "REVISION_MISMATCH") {
        const resolved = await this.#resolveConflict(scope, draftId, api);
        this.#status.set(draftId, resolved ? "saved" : "error");
      } else {
        // Keep the local text; the user can retry or the next edit retries.
        this.#status.set(draftId, "error");
      }
    }
  }

  /**
   * Keeps both sides of a concurrent edit: adopt the server copy and save the
   * local body as a separate draft, so no typed text is lost.
   */
  async #resolveConflict(scope: string, draftId: string, api: ApiClient): Promise<boolean> {
    const local = this.list(scope).find((draft) => draft.draft_id === draftId);
    if (local === undefined) return false;

    let server: Draft | undefined;
    try {
      const drafts = await this.#fetch(api, local.session_id);
      server = drafts.find((draft) => draft.draft_id === draftId);
    } catch {
      return false;
    }
    if (server === undefined) {
      // Deleted on another device: drop the local copy.
      this.#drafts.set(
        scope,
        this.list(scope).filter((draft) => draft.draft_id !== draftId),
      );
      return true;
    }
    if (server.body === local.body && server.title === local.title) {
      this.#replace(scope, server);
      return true;
    }

    let copy: Draft;
    try {
      copy = await this.#createRemote(api, local.session_id, local.body, {
        projectId: local.project_id,
        nodeId: local.node_id,
      });
    } catch {
      return false;
    }
    this.#replace(scope, server);
    this.#drafts.set(scope, [...this.list(scope), copy]);
    this.select(scope, copy.draft_id);
    toast.info(m.drafts_conflict_copy());
    return true;
  }

  #replace(scope: string, draft: Draft): void {
    const drafts = this.list(scope).map((item) =>
      item.draft_id === draft.draft_id ? draft : item,
    );
    this.#drafts.set(scope, drafts);
  }

  async #deleteRemote(api: ApiClient, draftId: string): Promise<void> {
    try {
      await api.deleteDraft(draftId);
    } catch {
      // Delete is idempotent and best effort; local state is still dropped.
    }
  }
}

export const composerDrafts = new ComposerDraftStore();
