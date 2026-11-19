import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as DropdownMenu from "./index.ts";

afterEach(cleanup);

class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;

describe("DropdownMenu", () => {
  it("uses a button trigger and closes when an item is selected", () => {
    const onSelect = vi.fn<() => void>();
    const onOpenChange = vi.fn<(open: boolean) => void>();
    const { container } = render(() => (
      <DropdownMenu.Root onOpenChange={onOpenChange}>
        <DropdownMenu.Trigger>Actions</DropdownMenu.Trigger>
        <DropdownMenu.Content>
          <DropdownMenu.Item onSelect={onSelect}>Rename</DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Root>
    ));

    const trigger = container.querySelector<HTMLButtonElement>(
      "[data-slot='dropdown-menu-trigger']",
    );
    expect(trigger?.getAttribute("type")).toBe("button");

    fireEvent.click(screen.getByText("Actions"));
    flush();
    const item = screen.getByText("Rename");
    fireEvent.click(item);
    flush();

    expect(onSelect).toHaveBeenCalledTimes(1);
    // Terracotta closes the panel; the wrapper must not emit a second close.
    expect(onOpenChange.mock.calls.filter(([open]) => open === false)).toHaveLength(1);
  });

  it("keeps the menu open when onSelect prevents the default", () => {
    const onOpenChange = vi.fn<(open: boolean) => void>();
    render(() => (
      <DropdownMenu.Root onOpenChange={onOpenChange}>
        <DropdownMenu.Trigger>Actions</DropdownMenu.Trigger>
        <DropdownMenu.Content>
          <DropdownMenu.Item onSelect={(event) => event.preventDefault()}>Stay</DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Root>
    ));

    fireEvent.click(screen.getByText("Actions"));
    flush();
    fireEvent.click(screen.getByText("Stay"));
    flush();

    expect(onOpenChange.mock.calls.filter(([open]) => open === false)).toHaveLength(0);
    expect(screen.getByText("Stay")).toBeTruthy();
  });
});
