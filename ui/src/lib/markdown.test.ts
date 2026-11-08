// @vitest-environment jsdom
// DOMPurify does not work correctly in happy-dom (an officially unsupported
// environment where tag/attribute sanitization misbehaves), so only this file
// runs under jsdom. Real browsers (production) are unaffected.
import { describe, expect, it } from "vitest";
import { renderMarkdown } from "./markdown";

describe("renderMarkdown", () => {
  it("converts headings, lists and emphasis to HTML", () => {
    const html = renderMarkdown("# Heading\n\n- a\n- **b**");
    expect(html).toContain("<h1>Heading</h1>");
    expect(html).toContain("<ul>");
    expect(html).toContain("<strong>b</strong>");
  });

  it("converts GFM tables", () => {
    const html = renderMarkdown("| a | b |\n| - | - |\n| 1 | 2 |");
    expect(html).toContain("<table>");
    expect(html).toContain("<td>1</td>");
  });

  it("adds a language class to code fences", () => {
    const html = renderMarkdown("```ts\nconst x = 1;\n```");
    expect(html).toContain("<pre>");
    expect(html).toContain("language-ts");
    expect(html).toContain("const x = 1;");
  });

  it("converts single newlines to <br> (breaks)", () => {
    expect(renderMarkdown("a\nb")).toContain("<br>");
  });

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

  it("returns empty HTML for an empty string", () => {
    expect(renderMarkdown("")).toBe("");
  });
});
