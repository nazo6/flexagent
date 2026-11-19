import { createRoot, createSignal, flush, type Accessor } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { createControllable } from "./controllable.ts";

describe("createControllable", () => {
  it("keeps its own state when uncontrolled and reports every change", () => {
    let value!: Accessor<string>;
    let setValue!: (next: string) => void;
    let dispose!: () => void;
    const onChange = vi.fn<(next: string) => void>();

    dispose = createRoot((disposeRoot) => {
      [value, setValue] = createControllable(
        () => undefined as string | undefined,
        () => onChange,
        () => "fallback",
      );
      return disposeRoot;
    });

    expect(value()).toBe("fallback");
    setValue("local");
    flush();
    expect(value()).toBe("local");
    expect(onChange).toHaveBeenCalledWith("local");

    // The uncontrolled value may be cleared back to `undefined`; the fallback
    // still supplies the default.
    setValue(undefined as unknown as string);
    flush();
    expect(value()).toBe("fallback");
    dispose();
  });

  it("follows the controlled value and switches modes dynamically", () => {
    let value!: Accessor<string>;
    let setValue!: (next: string) => void;
    let setExternal!: (next: string | undefined) => void;
    const onChange = vi.fn<(next: string) => void>();

    const dispose = createRoot((disposeRoot) => {
      const [external, writeExternal] = createSignal<string | undefined>(undefined);
      setExternal = writeExternal;
      [value, setValue] = createControllable(
        external,
        () => onChange,
        () => "unset",
      );
      return disposeRoot;
    });

    expect(value()).toBe("unset");
    setExternal("remote");
    flush();
    expect(value()).toBe("remote");

    setValue("requested");
    flush();
    // The caller owns the state: the write is reported but not mirrored.
    expect(value()).toBe("remote");
    expect(onChange).toHaveBeenCalledWith("requested");

    setExternal(undefined);
    flush();
    expect(value()).toBe("requested");
    dispose();
  });
});
