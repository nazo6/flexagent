// @vitest-environment jsdom
// The copy decoration builds real DOM nodes, so run it under jsdom like the
// Markdown sanitizer tests.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { enhanceCodeBlocks } from "./highlight.ts";
import { renderMarkdown } from "./markdown.ts";

describe("enhanceCodeBlocks", () => {
  beforeEach(() => {
    // jsdom has no matchMedia; the highlighter only reads the preferred scheme.
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: () => ({ matches: false }),
    });
  });

  it("wraps a fenced block and copies its source", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    const root = document.createElement("div");
    root.innerHTML = renderMarkdown("```\nplain block\n```");
    await enhanceCodeBlocks(root);

    const wrapper = root.querySelector(".fxg-code");
    expect(wrapper).not.toBeNull();
    const button = wrapper?.querySelector<HTMLButtonElement>("button.fxg-copy");
    expect(button?.textContent).toBe("Copy");

    button?.click();
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith("plain block\n"));
    await vi.waitFor(() => expect(button?.textContent).toBe("Copied"));
  });

  it("decorates a block with an unknown language instead of dropping it", async () => {
    const root = document.createElement("div");
    root.innerHTML = renderMarkdown("```not-a-language\nkeep me\n```");
    await enhanceCodeBlocks(root);

    const wrapper = root.querySelector(".fxg-code");
    expect(wrapper).not.toBeNull();
    expect(wrapper?.textContent).toContain("keep me");
    expect(wrapper?.querySelector("button.fxg-copy")).not.toBeNull();
  });
});
