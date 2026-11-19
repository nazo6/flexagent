import { cleanup, fireEvent, render } from "@solidjs/testing-library";
import { flush } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import * as Tabs from "./index.ts";

afterEach(cleanup);

describe("Tabs", () => {
  it("switches an uncontrolled group from defaultValue", () => {
    const { getByText } = render(() => (
      <Tabs.Root defaultValue="one">
        <Tabs.List>
          <Tabs.Trigger value="one">One</Tabs.Trigger>
          <Tabs.Trigger value="two">Two</Tabs.Trigger>
        </Tabs.List>
        <Tabs.Content value="one">Panel one</Tabs.Content>
        <Tabs.Content value="two">Panel two</Tabs.Content>
      </Tabs.Root>
    ));

    const one = getByText("One");
    const two = getByText("Two");
    expect(one.hasAttribute("tc-selected")).toBe(true);

    fireEvent.click(two);
    flush();
    expect(two.hasAttribute("tc-selected")).toBe(true);
    expect(one.hasAttribute("tc-selected")).toBe(false);
  });

  it("keeps an explicitly controlled value in sync with the caller", () => {
    const onValueChange = (value: string | undefined) => {
      const group = document.querySelector("[data-slot='tabs']");
      group?.setAttribute("data-last-change", value ?? "");
    };
    const { getByText } = render(() => (
      <Tabs.Root value="two" onValueChange={onValueChange}>
        <Tabs.List>
          <Tabs.Trigger value="one">One</Tabs.Trigger>
          <Tabs.Trigger value="two">Two</Tabs.Trigger>
        </Tabs.List>
        <Tabs.Content value="one">Panel one</Tabs.Content>
        <Tabs.Content value="two">Panel two</Tabs.Content>
      </Tabs.Root>
    ));

    fireEvent.click(getByText("One"));
    flush();
    expect(document.querySelector("[data-slot='tabs']")?.getAttribute("data-last-change")).toBe(
      "one",
    );
  });
});
