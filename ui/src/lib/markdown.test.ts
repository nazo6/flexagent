// @vitest-environment jsdom
// DOMPurify は happy-dom で正しく動作しない (公式に未サポート環境。タグ・属性の
// サニタイズが誤動作する) ため、このファイルだけ jsdom で実行する。
// 実ブラウザ (本番) では問題ない。
import { describe, expect, it } from "vitest";
import { renderMarkdown } from "./markdown";

describe("renderMarkdown", () => {
  it("見出し・リスト・強調を HTML へ変換する", () => {
    const html = renderMarkdown("# 見出し\n\n- a\n- **b**");
    expect(html).toContain("<h1>見出し</h1>");
    expect(html).toContain("<ul>");
    expect(html).toContain("<strong>b</strong>");
  });

  it("GFM テーブルを変換する", () => {
    const html = renderMarkdown("| a | b |\n| - | - |\n| 1 | 2 |");
    expect(html).toContain("<table>");
    expect(html).toContain("<td>1</td>");
  });

  it("コードフェンスへ言語クラスを付与する", () => {
    const html = renderMarkdown("```ts\nconst x = 1;\n```");
    expect(html).toContain("<pre>");
    expect(html).toContain("language-ts");
    expect(html).toContain("const x = 1;");
  });

  it("単一改行を <br> へ変換する (breaks)", () => {
    expect(renderMarkdown("a\nb")).toContain("<br>");
  });

  it("script / イベントハンドラを除去する", () => {
    const html = renderMarkdown("<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>");
    expect(html).not.toContain("<script");
    expect(html).not.toContain("onerror");
  });

  it("javascript: URL を無害化する", () => {
    const html = renderMarkdown("[x](javascript:alert(1))");
    expect(html).not.toContain("javascript:");
  });

  it("リンクへ target=_blank / rel=noopener を付与する", () => {
    const html = renderMarkdown("[example](https://example.com)");
    expect(html).toContain('target="_blank"');
    expect(html).toContain('rel="noopener noreferrer"');
  });

  it("空文字は空 HTML を返す", () => {
    expect(renderMarkdown("")).toBe("");
  });
});
