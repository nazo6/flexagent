// DOMPurify needs a real DOM (jsdom is the configured test environment).
import { createSignal } from "solid-js";
import { fireEvent, render, waitFor } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import MarkdownText from "./MarkdownText.tsx";
import { connection } from "#lib/stores/app.ts";
import { dock } from "#lib/stores/dock.ts";

function renderText(text: string, fileBaseDir: string | null = null): HTMLElement {
  return render(() => <MarkdownText text={text} fileBaseDir={fileBaseDir} />).container;
}

function anchorIn(target: HTMLElement): HTMLAnchorElement {
  const anchor = target.querySelector("a");
  if (anchor === null) throw new Error("missing anchor");
  return anchor;
}

/**
 * Dispatch the click through `fireEvent` (navigating to a link would make jsdom
 * log an unimplemented navigation) and keep the event for `defaultPrevented`:
 * the component must cancel the browser navigation to open the file instead.
 */
function click(anchor: HTMLElement, button = 0): MouseEvent {
  const event = new MouseEvent(button === 0 ? "click" : "auxclick", {
    bubbles: true,
    cancelable: true,
    button,
  });
  fireEvent(anchor, event);
  return event;
}

beforeEach(() => {
  localStorage.clear();
  dock.workspaces = {};
  dock.currentKey = null;
  dock.setContext({ sessionId: "s1", nodeId: "node-1", localPath: "/repo" });
  connection.role = "local_node";
});

afterEach(() => {
  document.body.replaceChildren();
  dock.setContext(null);
  connection.role = null;
});

describe("MarkdownText file links", () => {
  it("opens a relative file link in the Files dock", () => {
    const target = renderText("[main](src/main.ts#L12)");
    const event = click(anchorIn(target));

    expect(event.defaultPrevented).toBe(true);
    expect(dock.activeTool).toBe("files");
    expect(dock.activeFile).toBe("/repo/src/main.ts");
    expect(dock.activeFileTab?.line).toBe(12);
  });

  it("uses the base directory for links inside a Markdown preview", () => {
    const target = renderText("[main](../src/main.ts)", "/repo/docs");
    click(anchorIn(target));

    expect(dock.activeFile).toBe("/repo/src/main.ts");
  });

  it("opens a file URI with a middle click", () => {
    const target = renderText("[main](file:///repo/src/main.ts)");
    const event = click(anchorIn(target), 1);

    expect(event.defaultPrevented).toBe(true);
    expect(dock.activeFile).toBe("/repo/src/main.ts");
  });

  it("opens a Windows path link", () => {
    dock.setContext({ sessionId: "s1", nodeId: "node-1", localPath: "C:\\repo" });
    const target = renderText("[main](C:\\repo\\src\\main.ts)");
    click(anchorIn(target));

    expect(dock.activeFile).toBe("C:\\repo\\src\\main.ts");
  });

  it("leaves web links to the browser", () => {
    const target = renderText("[site](https://example.com)");
    const event = click(anchorIn(target));

    expect(event.defaultPrevented).toBe(false);
    expect(dock.activeFile).toBeNull();
  });

  it("opens a relative link when the node is reached through the hub", () => {
    connection.role = "hub";
    const target = renderText("[main](src/main.ts)");
    const event = click(anchorIn(target));

    expect(event.defaultPrevented).toBe(true);
    expect(dock.activeFile).toBe("/repo/src/main.ts");
  });
});

describe("MarkdownText rendering", () => {
  it("re-renders when the streaming text changes", async () => {
    // Solid port of the Svelte reactivity assertion: the rendered text must
    // follow `props.text`, otherwise a streamed message would stay at its first
    // chunk.
    const [text, setText] = createSignal("first");
    const { container } = render(() => <MarkdownText text={text()} />);

    expect(container.textContent).toContain("first");
    setText("second chunk");
    await waitFor(() => expect(container.textContent).toContain("second chunk"));
    expect(container.textContent).not.toContain("first");
  });
});
