import { afterEach, describe, expect, it, vi } from "vitest";
import { copyText } from "./clipboard";

function stubExecCommand(result: boolean) {
  const execCommand = vi.fn<(command: string) => boolean>((command) => {
    expect(command).toBe("copy");
    return result;
  });
  Object.defineProperty(document, "execCommand", { value: execCommand, configurable: true });
  return execCommand;
}

describe("copyText", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    Reflect.deleteProperty(document, "execCommand");
  });

  it("uses the async Clipboard API when it is available", async () => {
    const writeText = vi.fn<() => Promise<void>>().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });

    await copyText("hello");

    expect(writeText).toHaveBeenCalledWith("hello");
  });

  it("falls back to execCommand when navigator.clipboard is unavailable (plain HTTP)", async () => {
    vi.stubGlobal("navigator", {});
    let copied: string | null = null;
    Object.defineProperty(document, "execCommand", {
      value: vi.fn<() => boolean>(() => {
        copied =
          document.activeElement instanceof HTMLTextAreaElement
            ? document.activeElement.value
            : null;
        return true;
      }),
      configurable: true,
    });

    await copyText("hello over http");

    expect(copied).toBe("hello over http");
    expect(document.querySelectorAll("textarea")).toHaveLength(0);
  });

  it("falls back to execCommand when the async copy rejects", async () => {
    const writeText = vi.fn<() => Promise<void>>().mockRejectedValue(new Error("denied"));
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    const execCommand = stubExecCommand(true);

    await copyText("hello");

    expect(writeText).toHaveBeenCalledWith("hello");
    expect(execCommand).toHaveBeenCalledTimes(1);
  });

  it("throws when neither copy path is available", async () => {
    vi.stubGlobal("navigator", {});
    stubExecCommand(false);

    await expect(copyText("hello")).rejects.toThrow("clipboard is unavailable");
  });
});
