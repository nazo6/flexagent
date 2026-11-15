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
});
