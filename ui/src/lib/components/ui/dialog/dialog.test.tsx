import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as Dialog from "./index.ts";

afterEach(cleanup);

describe("Dialog", () => {
  it("renders an open dialog and reports close requests", () => {
    const onOpenChange = vi.fn<(open: boolean) => void>();
    const { container } = render(() => (
      <Dialog.Root open onOpenChange={onOpenChange}>
        <Dialog.Content>
          <Dialog.Title>Settings</Dialog.Title>
          <Dialog.Description>Description</Dialog.Description>
          <Dialog.Close>Cancel</Dialog.Close>
        </Dialog.Content>
      </Dialog.Root>
    ));

    expect(screen.getByText("Settings")).toBeTruthy();
    expect(screen.getByText("Description")).toBeTruthy();
    expect(
      container.ownerDocument.body.querySelector("[data-slot='dialog-overlay']"),
    ).not.toBeNull();

    fireEvent.click(screen.getByText("Cancel"));
    flush();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("does not open from a disabled trigger", () => {
    const onOpenChange = vi.fn<(open: boolean) => void>();
    render(() => (
      <Dialog.Root onOpenChange={onOpenChange}>
        <Dialog.Trigger disabled>Open</Dialog.Trigger>
      </Dialog.Root>
    ));

    fireEvent.click(screen.getByText("Open"));
    flush();
    expect(onOpenChange).not.toHaveBeenCalled();
  });
});
