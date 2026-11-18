// @vitest-environment jsdom
// DOMPurify needs a real DOM (see markdown.test.ts).
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import MarkdownText from "./MarkdownText.svelte";
import { dock } from "#lib/stores/dock.svelte.ts";
import { sync } from "#lib/stores/app.svelte.ts";

let component: ReturnType<typeof mount> | null = null;

function render(text: string, baseDir: string | null = null): HTMLElement {
  const target = document.createElement("div");
  document.body.append(target);
  component = mount(MarkdownText, { target, props: { text, fileBaseDir: baseDir } });
  flushSync();
  return target;
}

function click(anchor: HTMLElement, button = 0): MouseEvent {
  const event = new MouseEvent(button === 0 ? "click" : "auxclick", {
    bubbles: true,
    cancelable: true,
    button,
  });
  anchor.dispatchEvent(event);
  return event;
}

beforeEach(() => {
  localStorage.clear();
  dock.workspaces = {};
  dock.currentKey = null;
  dock.setContext({ sessionId: "s1", nodeId: "node-1", localPath: "/repo" });
  sync.connection.role = "local_node";
});

afterEach(() => {
  if (component !== null) unmount(component);
  component = null;
  document.body.replaceChildren();
  dock.setContext(null);
  sync.connection.role = null;
});

describe("MarkdownText file links", () => {
  it("opens a relative file link in the Files dock", () => {
    const target = render("[main](src/main.ts#L12)");
    const anchor = target.querySelector("a");
    if (anchor === null) throw new Error("missing anchor");

    const event = click(anchor);

    expect(event.defaultPrevented).toBe(true);
    expect(dock.activeTool).toBe("files");
    expect(dock.activeFile).toBe("/repo/src/main.ts");
    expect(dock.activeFileTab?.line).toBe(12);
  });

  it("uses the base directory for links inside a Markdown preview", () => {
    const target = render("[main](../src/main.ts)", "/repo/docs");
    const anchor = target.querySelector("a");
    if (anchor === null) throw new Error("missing anchor");

    click(anchor);

    expect(dock.activeFile).toBe("/repo/src/main.ts");
  });

  it("opens a file URI with a middle click", () => {
    const target = render("[main](file:///repo/src/main.ts)");
    const anchor = target.querySelector("a");
    if (anchor === null) throw new Error("missing anchor");

    const event = click(anchor, 1);

    expect(event.defaultPrevented).toBe(true);
    expect(dock.activeFile).toBe("/repo/src/main.ts");
  });

  it("opens a Windows path link", () => {
    dock.setContext({ sessionId: "s1", nodeId: "node-1", localPath: "C:\\repo" });
    const target = render("[main](C:\\repo\\src\\main.ts)");
    const anchor = target.querySelector("a");
    if (anchor === null) throw new Error("missing anchor");

    click(anchor);

    expect(dock.activeFile).toBe("C:\\repo\\src\\main.ts");
  });

  it("leaves web links to the browser", () => {
    const target = render("[site](https://example.com)");
    const anchor = target.querySelector("a");
    if (anchor === null) throw new Error("missing anchor");

    const event = click(anchor);

    expect(event.defaultPrevented).toBe(false);
    expect(dock.activeFile).toBeNull();
  });

  it("opens a relative link when the node is reached through the hub", () => {
    sync.connection.role = "hub";
    const target = render("[main](src/main.ts)");
    const anchor = target.querySelector("a");
    if (anchor === null) throw new Error("missing anchor");

    const event = click(anchor);

    expect(event.defaultPrevented).toBe(true);
    expect(dock.activeFile).toBe("/repo/src/main.ts");
  });
});
