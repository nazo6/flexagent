import { describe, expect, it, vi } from "vitest";
import { GhosttyWebAdapter } from "./ghostty-adapter";

vi.mock("ghostty-web", () => {
  /** Minimal stand-in for the parts of `Terminal` the adapter touches. */
  class FakeTerminal {
    cols = 80;
    rows = 24;
    buffer = { active: { cursorX: 2, cursorY: 1 } };
    open(element: HTMLElement): void {
      element.setAttribute("contenteditable", "true");
      const canvas = document.createElement("canvas");
      const input = document.createElement("textarea");
      input.style.opacity = "0";
      element.append(canvas, input);
    }
    onData(): { dispose(): void } {
      return { dispose() {} };
    }
    onResize(): { dispose(): void } {
      return { dispose() {} };
    }
    write(): void {}
    clear(): void {}
    focus(): void {}
    loadAddon(): void {}
    dispose(): void {}
  }
  class FakeFitAddon {
    fit(): void {}
    observeResize(): void {}
    dispose(): void {}
  }
  return {
    init: () => Promise.resolve(),
    Terminal: FakeTerminal,
    FitAddon: FakeFitAddon,
  };
});

/** Mounts the adapter into a fresh container and returns its DOM. */
async function mountAdapter(): Promise<{
  adapter: GhosttyWebAdapter;
  element: HTMLElement;
  input: HTMLTextAreaElement;
}> {
  const element = document.createElement("div");
  document.body.append(element);
  const adapter = new GhosttyWebAdapter();
  adapter.mount(element);
  await vi.waitFor(() => expect(element.querySelector("textarea")).not.toBeNull());
  const input = element.querySelector("textarea");
  if (input === null) throw new Error("the fake terminal did not create its textarea");
  return { adapter, element, input };
}

/** Builds a `beforeinput` event with the fields the adapter reads. */
function beforeInput(data: string | null, inputType: string, isComposing = false): InputEvent {
  const event = new Event("beforeinput", { bubbles: true, cancelable: true });
  Object.assign(event, { data, inputType, isComposing });
  return event as InputEvent;
}

/** Builds the `input` event the browser fires after inserting text. */
function insertedText(isComposing = false): InputEvent {
  const event = new Event("input", { bubbles: true });
  Object.assign(event, { isComposing });
  return event as InputEvent;
}

/** Builds a composition event carrying `data` (happy-dom drops it otherwise). */
function composition(type: "compositionstart" | "compositionend", data: string): CompositionEvent {
  const event = new CompositionEvent(type, { bubbles: true });
  Object.defineProperty(event, "data", { value: data });
  return event;
}

describe("GhosttyWebAdapter input target", () => {
  it("focuses the hidden textarea rather than the wrapper", async () => {
    const { adapter, element, input } = await mountAdapter();

    adapter.focus();

    expect(document.activeElement).toBe(input);
    expect(document.activeElement).not.toBe(element);
  });

  it("moves focus that reaches the contenteditable wrapper into the textarea", async () => {
    const { adapter, element, input } = await mountAdapter();

    element.focus();
    expect(document.activeElement).toBe(input);

    element.blur();
    adapter.focus();
    expect(document.activeElement).toBe(input);
  });

  it("sends text the browser inserts while an IME owns the keys", async () => {
    const { adapter, input } = await mountAdapter();
    const sent: string[] = [];
    adapter.onData((data) => sent.push(data));

    input.value = "ls";
    input.dispatchEvent(insertedText());

    expect(sent).toEqual(["ls"]);
    expect(input.value).toBe("");
  });

  it("keeps the preedit in the field and out of the terminal", async () => {
    const { adapter, input } = await mountAdapter();
    const sent: string[] = [];
    adapter.onData((data) => sent.push(data));

    input.dispatchEvent(composition("compositionstart", ""));
    expect(input.style.opacity).toBe("1");
    expect(input.style.clipPath).toBe("none");

    input.value = "にほ";
    input.dispatchEvent(insertedText(true));
    expect(sent).toEqual([]);
    expect(input.value).toBe("にほ");

    input.value = "にほん";
    input.dispatchEvent(composition("compositionend", "にほん"));
    expect(input.style.opacity).toBe("0");
    expect(input.style.clipPath).toBe("inset(50%)");
  });

  it("sends a committed composition exactly once whatever the event order", async () => {
    const { adapter, input } = await mountAdapter();
    const sent: string[] = [];
    adapter.onData((data) => sent.push(data));

    // `input` first (the usual browser order), then `compositionend`.
    input.value = "日本";
    input.dispatchEvent(insertedText());
    input.dispatchEvent(composition("compositionend", "日本"));
    await Promise.resolve();
    expect(sent).toEqual(["日本"]);

    // `compositionend` first: the commit is picked up from the field.
    input.value = "かん";
    input.dispatchEvent(composition("compositionend", "かん"));
    await Promise.resolve();
    expect(sent).toEqual(["日本", "かん"]);
  });

  it("stops beforeinput at the textarea so the wrapper cannot cancel it", async () => {
    const { element, input } = await mountAdapter();
    const reachedWrapper: string[] = [];
    element.addEventListener("beforeinput", (event) => reachedWrapper.push(event.type));

    const event = beforeInput("ls", "insertText");
    input.dispatchEvent(event);

    expect(reachedWrapper).toEqual([]);
    expect(event.defaultPrevented).toBe(false);
  });

  it("forwards editing keys the IME reports as beforeinput edits", async () => {
    const { adapter, input } = await mountAdapter();
    const sent: string[] = [];
    adapter.onData((data) => sent.push(data));

    const event = beforeInput(null, "deleteContentBackward");
    input.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(true);
    expect(sent).toEqual(["\x7f"]);
  });
});
