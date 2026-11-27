import { describe, expect, it } from "vitest";
import { createComposerAttachments } from "./composer-attachments.svelte.ts";

const file = {
  file_name: "a.txt",
  mime_type: "text/plain",
  size_bytes: 0,
  local_path: "/tmp/a.txt",
  data_b64: null,
};

describe("createComposerAttachments", () => {
  it("notifies only user-driven mutations", () => {
    const changes: number[] = [];
    const composer = createComposerAttachments({ onchange: (next) => changes.push(next.length) });

    composer.addLocalFile("/tmp/a.txt");
    expect(changes).toEqual([1]);

    // Restoring a draft is not an edit; it must not echo back into the store.
    composer.replace([file], false);
    expect(changes).toEqual([1]);

    composer.remove(0);
    expect(changes).toEqual([1, 0]);
  });

  it("notifies when a replacement is an edit", () => {
    const changes: number[] = [];
    const composer = createComposerAttachments({ onchange: (next) => changes.push(next.length) });

    composer.replace([file]);
    expect(changes).toEqual([1]);
    expect(composer.attachments).toEqual([file]);
  });

  it("clears silently when the draft is being discarded", () => {
    const changes: number[] = [];
    const composer = createComposerAttachments({ onchange: (next) => changes.push(next.length) });

    composer.addLocalFile("/tmp/a.txt");
    composer.clear(false);
    expect(changes).toEqual([1]);
    expect(composer.attachments).toEqual([]);
  });
});
