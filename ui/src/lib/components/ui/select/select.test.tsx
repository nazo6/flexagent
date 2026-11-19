import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as Select from "./index.ts";

afterEach(cleanup);

// `Select.Content` measures itself and the floating panel with ResizeObserver,
// which jsdom does not provide.
class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;

describe("Select", () => {
  it("renders the registered label and a non-submitting trigger", () => {
    const { container } = render(() => (
      <Select.Root value="b">
        <Select.Trigger id="pick">trigger</Select.Trigger>
        <Select.Content>
          <Select.Item value="a" label="Alpha">
            Alpha
          </Select.Item>
          <Select.Item value="b" label="Beta">
            Beta
          </Select.Item>
        </Select.Content>
      </Select.Root>
    ));

    const trigger = container.querySelector<HTMLButtonElement>("[data-slot='select-trigger']");
    expect(trigger).not.toBeNull();
    expect(trigger?.getAttribute("type")).toBe("button");
    expect(screen.getByText("Beta")).toBeTruthy();
  });

  it("reports the selected value", () => {
    const onValueChange = vi.fn<(value: unknown) => void>();
    render(() => (
      <Select.Root defaultValue="a" defaultOpen onValueChange={onValueChange}>
        <Select.Trigger>pick</Select.Trigger>
        <Select.Content>
          <Select.Item value="a" label="Alpha">
            Alpha
          </Select.Item>
          <Select.Item value="b" label="Beta">
            Beta
          </Select.Item>
        </Select.Content>
      </Select.Root>
    ));
    flush();
    fireEvent.click(screen.getByText("Beta"));
    flush();
    expect(onValueChange).toHaveBeenCalledWith("b");
  });
});
