import { cleanup, fireEvent, render } from "@solidjs/testing-library";
import { createSignal, flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Input } from "./index.ts";

afterEach(cleanup);

describe("Input", () => {
  it("leaves an uncontrolled value alone when other props change", () => {
    const [placeholder, setPlaceholder] = createSignal("first");
    const { container } = render(() => <Input defaultValue="typed" placeholder={placeholder()} />);
    const input = container.querySelector("input");
    if (input === null) throw new Error("input missing");

    setPlaceholder("second");
    flush();
    expect(input.placeholder).toBe("second");
    expect(input.value).toBe("typed");
  });

  it("binds a controlled value and reports edits", () => {
    const [value, setValue] = createSignal("a");
    const onChange = vi.fn<(value: string) => void>();
    const { container } = render(() => <Input value={value()} onValueChange={onChange} />);
    const input = container.querySelector("input");
    if (input === null) throw new Error("input missing");

    expect(input.value).toBe("a");
    fireEvent.input(input, { target: { value: "b" } });
    expect(onChange).toHaveBeenCalledWith("b");

    setValue("c");
    flush();
    expect(input.value).toBe("c");
  });
});
