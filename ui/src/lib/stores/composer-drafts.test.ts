import { describe, expect, it } from "vitest";
import type { AttachmentMeta } from "#lib/generated/bindings.ts";
import { ComposerDraftStore, loadComposerDraft, saveComposerDraft } from "./composer-drafts.ts";

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

const attachment: AttachmentMeta = {
  file_name: "image.png",
  mime_type: "image/png",
  size_bytes: 4,
  local_path: null,
  data_b64: "AAAA",
};

describe("composer drafts", () => {
  it("round-trips prompt text per composer", () => {
    const storage = memoryStorage();
    saveComposerDraft("session-a", "first", storage);
    saveComposerDraft("session-b", "second", storage);
    expect(loadComposerDraft("session-a", storage)).toBe("first");
    expect(loadComposerDraft("session-b", storage)).toBe("second");
    expect(loadComposerDraft("session-c", storage)).toBe("");

    saveComposerDraft("session-a", "", storage);
    expect(loadComposerDraft("session-a", storage)).toBe("");
    expect(loadComposerDraft("session-b", storage)).toBe("second");
  });

  it("keeps text and attachments across store instances", () => {
    const storage = memoryStorage();
    const store = new ComposerDraftStore(storage);
    store.setText("session-a", "draft prompt");
    store.setAttachments("session-a", [attachment]);
    expect(store.get("session-a")).toEqual({
      text: "draft prompt",
      attachments: [attachment],
    });

    // A new instance simulates remounting the app: text is restored, but
    // attachments are intentionally memory-only.
    const reloaded = new ComposerDraftStore(storage);
    expect(reloaded.get("session-a")).toEqual({
      text: "draft prompt",
      attachments: [],
    });
  });

  it("drops the draft once both text and attachments are empty", () => {
    const storage = memoryStorage();
    const store = new ComposerDraftStore(storage);
    store.setText("session-a", "draft prompt");
    store.setAttachments("session-a", [attachment]);

    store.setText("session-a", "");
    expect(store.get("session-a").attachments).toEqual([attachment]);

    store.setAttachments("session-a", []);
    expect(store.get("session-a")).toEqual({ text: "", attachments: [] });
    expect(storage.getItem("fxg:composer_draft:session-a")).toBeNull();
  });

  it("clears a composer draft explicitly", () => {
    const storage = memoryStorage();
    const store = new ComposerDraftStore(storage);
    store.setText("session-a", "draft prompt");
    store.setAttachments("session-a", [attachment]);

    store.clear("session-a");
    expect(store.get("session-a")).toEqual({ text: "", attachments: [] });
    expect(loadComposerDraft("session-a", storage)).toBe("");
  });

  it("ignores empty composer ids and unavailable storage", () => {
    const storage = memoryStorage();
    saveComposerDraft("", "text", storage);
    expect(storage.length).toBe(0);
    expect(loadComposerDraft("", storage)).toBe("");
    expect(loadComposerDraft("session-a", null)).toBe("");
    saveComposerDraft("session-a", "text", null);
    expect(loadComposerDraft("session-a", storage)).toBe("");
  });
});
