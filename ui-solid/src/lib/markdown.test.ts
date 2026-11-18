// @vitest-environment jsdom
// DOMPurify does not work correctly in happy-dom (an officially unsupported
// environment where tag/attribute sanitization misbehaves), so only this file
// runs under jsdom. Real browsers (production) are unaffected.
import { describe, expect, it } from "vitest";
import { renderMarkdown } from "./markdown";

describe("renderMarkdown", () => {
  it("removes scripts and event handlers", () => {
    const html = renderMarkdown("<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>");
    expect(html).not.toContain("<script");
    expect(html).not.toContain("onerror");
  });

  it("neutralizes javascript: URLs", () => {
    const html = renderMarkdown("[x](javascript:alert(1))");
    expect(html).not.toContain("javascript:");
  });

  it("adds target=_blank / rel=noopener to links", () => {
    const html = renderMarkdown("[example](https://example.com)");
    expect(html).toContain('target="_blank"');
    expect(html).toContain('rel="noopener noreferrer"');
  });

  it("keeps file URIs and drive paths for the Files dock", () => {
    expect(renderMarkdown("[a](file:///repo/src/a.ts)")).toContain('href="file:///repo/src/a.ts"');
    expect(renderMarkdown("[a](C:/repo/src/a.ts)")).toContain('href="C:/repo/src/a.ts"');
    expect(renderMarkdown("[a](C:\\repo\\src\\a.ts)")).toContain('href="C:%5Crepo%5Csrc%5Ca.ts"');
  });

  it("drops script-capable URI schemes", () => {
    const html = renderMarkdown("[a](javascript:alert(1)) [b](vbscript:x) [c](data:text/html,x)");
    expect(html).not.toContain("javascript:");
    expect(html).not.toContain("vbscript:");
    expect(html).not.toContain("data:");
  });
});
