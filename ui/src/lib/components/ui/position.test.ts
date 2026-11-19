import { createRoot, createSignal, flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { composeRef, dismissOnOutsidePointer } from "./position.ts";

describe("composeRef", () => {
  it("forwards the element to every ref", () => {
    const first = vi.fn<(element: HTMLElement) => void>();
    const second = vi.fn<(element: HTMLElement) => void>();
    const element = document.createElement("div");
    composeRef<HTMLDivElement>(first, second)(element);
    expect(first).toHaveBeenCalledWith(element);
    expect(second).toHaveBeenCalledWith(element);
  });
});

describe("dismissOnOutsidePointer", () => {
  it("dismisses only pointer presses outside the panel and the anchor", () => {
    createRoot((dispose) => {
      const [open] = createSignal(true);
      const anchor = document.createElement("button");
      const panel = document.createElement("div");
      panel.setAttribute("data-anchored-panel", "");
      document.body.append(anchor, panel);

      const onDismiss = vi.fn<() => void>();
      dismissOnOutsidePointer({ open, anchor: () => anchor, onDismiss });
      flush();

      panel.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
      anchor.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
      expect(onDismiss).not.toHaveBeenCalled();

      document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
      expect(onDismiss).toHaveBeenCalledTimes(1);

      dispose();
      anchor.remove();
      panel.remove();
    });
  });
});
