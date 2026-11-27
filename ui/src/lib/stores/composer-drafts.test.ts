import { describe, expect, it } from "vitest";
import type { ApiClient } from "#lib/api/client.ts";
import { ApiError } from "#lib/api/errors.ts";
import type { CreateDraftRequest, Draft, UpdateDraftRequest } from "#lib/generated/bindings.ts";
import {
  ComposerDraftStore,
  draftScope,
  draftTitle,
  loadActiveDraftId,
  loadDeviceId,
  NEW_SESSION_SCOPE,
  saveActiveDraftId,
} from "./composer-drafts.ts";

/** Minimal Storage implementation for tests (mimics localStorage in Node). */
function memoryStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (key: string) => map.get(key) ?? null,
    key: (index: number) => [...map.keys()][index] ?? null,
    removeItem: (key: string) => void map.delete(key),
    setItem: (key: string, value: string) => void map.set(key, value),
  };
}

function draft(overrides: Partial<Draft> = {}): Draft {
  const now = Date.now();
  return {
    draft_id: `draft-${Math.random().toString(36).slice(2)}`,
    title: "",
    body: "",
    session_id: null,
    project_id: null,
    node_id: null,
    attachments: [],
    revision: 1,
    origin_device_id: "device-test",
    created_at: now,
    updated_at: now,
    ...overrides,
  };
}

/** In-memory API that mirrors the revision checks of the real endpoints. */
function fakeApi(initial: Draft[] = []): ApiClient {
  const drafts = [...initial];
  const api = {
    async drafts(filter: { sessionId?: string | null; sessionless?: boolean }) {
      return drafts.filter((item) => {
        if (filter.sessionId !== undefined) return item.session_id === filter.sessionId;
        if (filter.sessionless === true) return item.session_id === null;
        return true;
      });
    },
    async draft(draftId: string) {
      const found = drafts.find((item) => item.draft_id === draftId);
      if (found === undefined) {
        throw new ApiError(404, { code: "NOT_FOUND", message: "missing" }, "missing");
      }
      return found;
    },
    async createDraft(request: CreateDraftRequest) {
      const created = draft({
        ...request,
        title: request.title ?? "",
        body: request.body ?? "",
        session_id: request.session_id ?? null,
        project_id: request.project_id ?? null,
        node_id: request.node_id ?? null,
        attachments: request.attachments ?? [],
      });
      drafts.push(created);
      return created;
    },
    async updateDraft(draftId: string, request: UpdateDraftRequest) {
      const index = drafts.findIndex((item) => item.draft_id === draftId);
      if (index < 0) throw new ApiError(404, { code: "NOT_FOUND", message: "missing" }, "missing");
      const current = drafts[index];
      if (current.revision !== request.expected_revision) {
        throw new ApiError(409, { code: "REVISION_MISMATCH", message: "conflict" }, "conflict");
      }
      const updated: Draft = {
        ...current,
        title: request.title ?? "",
        body: request.body ?? "",
        session_id: request.session_id ?? null,
        project_id: request.project_id ?? null,
        node_id: request.node_id ?? null,
        revision: current.revision + 1,
        updated_at: Date.now(),
      };
      drafts[index] = updated;
      return updated;
    },
    async deleteDraft(draftId: string) {
      const index = drafts.findIndex((item) => item.draft_id === draftId);
      if (index >= 0) drafts.splice(index, 1);
    },
  };
  return api as unknown as ApiClient;
}

describe("composer draft helpers", () => {
  it("maps session IDs to scopes", () => {
    expect(draftScope(null)).toBe(NEW_SESSION_SCOPE);
    expect(draftScope("")).toBe(NEW_SESSION_SCOPE);
    expect(draftScope("session-a")).toBe("session:session-a");
  });

  it("derives the title from the first non-empty line", () => {
    expect(draftTitle({ title: "", body: "\n  first line  \nsecond" })).toBe("first line");
    expect(draftTitle({ title: "Custom", body: "body" })).toBe("Custom");
    expect(draftTitle({ title: "", body: "" })).not.toBe("");
  });

  it("persists the device ID and the selected draft per scope", () => {
    const storage = memoryStorage();
    const first = loadDeviceId(storage);
    expect(loadDeviceId(storage)).toBe(first);
    expect(first).not.toBe("");

    saveActiveDraftId("session:a", "draft-1", storage);
    expect(loadActiveDraftId("session:a", storage)).toBe("draft-1");
    expect(loadActiveDraftId("session:b", storage)).toBeNull();
  });
});

describe("ComposerDraftStore", () => {
  it("creates the first draft and migrates the legacy localStorage text", async () => {
    const storage = memoryStorage();
    storage.setItem("fxg:composer_draft:session-a", "legacy prompt");
    const store = new ComposerDraftStore(storage);
    const api = fakeApi();

    const active = await store.ensure("session:session-a", "session-a", api);

    expect(active?.body).toBe("legacy prompt");
    expect(store.list("session:session-a")).toHaveLength(1);
    expect(storage.getItem("fxg:composer_draft:session-a")).toBeNull();
    expect(loadActiveDraftId("session:session-a", storage)).toBe(active?.draft_id);
  });

  it("keeps multiple drafts per scope and switches between them", async () => {
    const store = new ComposerDraftStore(memoryStorage());
    const api = fakeApi();
    const first = await store.ensure("session:session-a", "session-a", api);
    const second = await store.create("session:session-a", "session-a", api);

    expect(store.list("session:session-a")).toHaveLength(2);
    expect(store.activeDraft("session:session-a")?.draft_id).toBe(second.draft_id);

    store.select("session:session-a", first?.draft_id ?? "");
    expect(store.activeDraft("session:session-a")?.draft_id).toBe(first?.draft_id);
  });

  it("autosaves the body with the observed revision", async () => {
    const store = new ComposerDraftStore(memoryStorage());
    const api = fakeApi();
    const active = await store.ensure("session:session-a", "session-a", api);
    if (active === null) throw new Error("draft missing");

    store.setText("session:session-a", active.draft_id, "updated body");
    await store.flush("session:session-a", active.draft_id);

    expect(store.activeDraft("session:session-a")?.body).toBe("updated body");
    expect(store.activeDraft("session:session-a")?.revision).toBe(2);
  });

  it("renames a draft and reports the save status", async () => {
    const store = new ComposerDraftStore(memoryStorage());
    const api = fakeApi();
    const active = await store.ensure("session:session-a", "session-a", api);
    if (active === null) throw new Error("draft missing");

    store.rename("session:session-a", active.draft_id, "Plan B");
    await store.flush("session:session-a", active.draft_id);

    expect(store.activeDraft("session:session-a")?.title).toBe("Plan B");
    expect(store.saveStatus(active.draft_id)).toBe("saved");
  });

  it("removes a draft and selects the remaining one", async () => {
    const store = new ComposerDraftStore(memoryStorage());
    const api = fakeApi();
    const first = await store.ensure("session:session-a", "session-a", api);
    const second = await store.create("session:session-a", "session-a", api);
    if (first === null) throw new Error("draft missing");

    await store.remove("session:session-a", second.draft_id, "session-a", api);

    expect(store.list("session:session-a")).toHaveLength(1);
    expect(store.activeDraft("session:session-a")?.draft_id).toBe(first.draft_id);
  });

  it("keeps the local edit as a copy on a revision conflict", async () => {
    const store = new ComposerDraftStore(memoryStorage());
    const remote = draft({ draft_id: "draft-1", session_id: "session-a", body: "remote" });
    const api = fakeApi([remote]);
    const active = await store.ensure("session:session-a", "session-a", api);
    if (active === null) throw new Error("draft missing");

    // Simulate another device saving first.
    await api.updateDraft("draft-1", {
      expected_revision: 1,
      title: "",
      body: "remote edit",
      session_id: "session-a",
      project_id: null,
      node_id: null,
      attachments: [],
      origin_device_id: "other-device",
    });

    store.setText("session:session-a", "draft-1", "local edit");
    await store.flush("session:session-a", "draft-1");

    expect(store.list("session:session-a")).toHaveLength(2);
    const activeAfter = store.activeDraft("session:session-a");
    expect(activeAfter?.body).toBe("local edit");
    expect(activeAfter?.draft_id).not.toBe("draft-1");
  });

  it("applies a remote update when the local copy is clean", async () => {
    const store = new ComposerDraftStore(memoryStorage());
    const remote = draft({ draft_id: "draft-1", session_id: "session-a", body: "remote" });
    const api = fakeApi([remote]);
    await store.ensure("session:session-a", "session-a", api);

    await api.updateDraft("draft-1", {
      expected_revision: 1,
      title: "",
      body: "remote edit",
      session_id: "session-a",
      project_id: null,
      node_id: null,
      attachments: [],
      origin_device_id: "other-device",
    });
    await store.applyChange("draft-1", 2, false);

    expect(store.activeDraft("session:session-a")?.body).toBe("remote edit");
    expect(store.activeDraft("session:session-a")?.revision).toBe(2);
  });

  it("does not overwrite pending local edits with a remote update", async () => {
    const store = new ComposerDraftStore(memoryStorage());
    const remote = draft({ draft_id: "draft-1", session_id: "session-a", body: "remote" });
    const api = fakeApi([remote]);
    await store.ensure("session:session-a", "session-a", api);

    store.setText("session:session-a", "draft-1", "local edit");
    await api.updateDraft("draft-1", {
      expected_revision: 1,
      title: "",
      body: "remote edit",
      session_id: "session-a",
      project_id: null,
      node_id: null,
      attachments: [],
      origin_device_id: "other-device",
    });
    await store.applyChange("draft-1", 2, false);

    expect(store.activeDraft("session:session-a")?.body).toBe("local edit");
  });

  it("replaces a draft deleted on another device", async () => {
    const store = new ComposerDraftStore(memoryStorage());
    const remote = draft({ draft_id: "draft-1", session_id: "session-a", body: "remote" });
    const api = fakeApi([remote]);
    await store.ensure("session:session-a", "session-a", api);

    await api.deleteDraft("draft-1");
    await store.applyChange("draft-1", 2, true);

    expect(store.list("session:session-a")).toHaveLength(1);
    expect(store.list("session:session-a")[0]?.draft_id).not.toBe("draft-1");
  });
});
