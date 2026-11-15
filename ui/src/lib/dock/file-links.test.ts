import { describe, expect, it } from "vitest";
import { resolveFileHref } from "./file-links.ts";

const ROOT = "/repo";

describe("resolveFileHref", () => {
  it("resolves relative paths against the base directory", () => {
    expect(resolveFileHref("src/main.ts", ROOT, ROOT)).toEqual({
      path: "/repo/src/main.ts",
      line: null,
    });
    expect(resolveFileHref("./src/../main.ts", ROOT, ROOT)).toEqual({
      path: "/repo/main.ts",
      line: null,
    });
    expect(resolveFileHref("../src/main.ts", "/repo/docs", ROOT)).toEqual({
      path: "/repo/src/main.ts",
      line: null,
    });
  });

  it("decodes percent-encoded paths", () => {
    expect(resolveFileHref("src/a%20b.ts", ROOT, ROOT)?.path).toBe("/repo/src/a b.ts");
  });

  it("accepts absolute paths inside the workspace only", () => {
    expect(resolveFileHref("/repo/src/main.ts", ROOT, ROOT)?.path).toBe("/repo/src/main.ts");
    expect(resolveFileHref("/etc/passwd", ROOT, ROOT)).toBeNull();
    expect(resolveFileHref("/repo-other/main.ts", ROOT, ROOT)).toBeNull();
    expect(resolveFileHref("../../etc/passwd", ROOT, ROOT)).toBeNull();
  });

  it("resolves file URIs", () => {
    expect(resolveFileHref("file:///repo/src/main.ts", ROOT, ROOT)?.path).toBe("/repo/src/main.ts");
    expect(resolveFileHref("file://localhost/repo/src/main.ts", ROOT, ROOT)?.path).toBe(
      "/repo/src/main.ts",
    );
    expect(resolveFileHref("file:///etc/passwd", ROOT, ROOT)).toBeNull();
    expect(resolveFileHref("file:///repo/src/", ROOT, ROOT)).toBeNull();
  });

  it("resolves Windows drive and UNC file URIs", () => {
    expect(resolveFileHref("file:///C:/repo/src/main.ts", "C:\\repo", "C:\\repo")?.path).toBe(
      "C:\\repo\\src\\main.ts",
    );
    expect(resolveFileHref("C:/repo/src/main.ts", "C:\\repo", "C:\\repo")?.path).toBe(
      "C:\\repo\\src\\main.ts",
    );
    // Marked percent-encodes backslashes in hrefs before sanitization.
    expect(resolveFileHref("C:%5Crepo%5Csrc%5Cmain.ts", "C:\\repo", "C:\\repo")?.path).toBe(
      "C:\\repo\\src\\main.ts",
    );
    expect(resolveFileHref("D:/other/main.ts", "C:\\repo", "C:\\repo")).toBeNull();
    expect(resolveFileHref("file://server/share/a.ts", "C:\\repo", "\\\\server\\share")?.path).toBe(
      "\\\\server\\share\\a.ts",
    );
  });

  it("reads line anchors and suffixes", () => {
    expect(resolveFileHref("src/main.ts#L12", ROOT, ROOT)).toEqual({
      path: "/repo/src/main.ts",
      line: 12,
    });
    expect(resolveFileHref("src/main.ts#L12-L20", ROOT, ROOT)).toEqual({
      path: "/repo/src/main.ts",
      line: 12,
    });
    expect(resolveFileHref("src/main.ts:42", ROOT, ROOT)).toEqual({
      path: "/repo/src/main.ts",
      line: 42,
    });
    expect(resolveFileHref("src/main.ts:42:7", ROOT, ROOT)).toEqual({
      path: "/repo/src/main.ts",
      line: 42,
    });
    expect(resolveFileHref("file:///repo/src/main.ts#L7", ROOT, ROOT)).toEqual({
      path: "/repo/src/main.ts",
      line: 7,
    });
    expect(resolveFileHref("src/main.ts#install", ROOT, ROOT)).toEqual({
      path: "/repo/src/main.ts",
      line: null,
    });
    expect(resolveFileHref("src/main.ts?plain=1", ROOT, ROOT)).toEqual({
      path: "/repo/src/main.ts",
      line: null,
    });
  });

  it("leaves non-file links alone", () => {
    expect(resolveFileHref("https://example.com/a.ts", ROOT, ROOT)).toBeNull();
    expect(resolveFileHref("mailto:a@example.com", ROOT, ROOT)).toBeNull();
    expect(resolveFileHref("#L12", ROOT, ROOT)).toBeNull();
    expect(resolveFileHref("//example.com/a.ts", ROOT, ROOT)).toBeNull();
    expect(resolveFileHref("src:42", ROOT, ROOT)).toBeNull();
    expect(resolveFileHref("", ROOT, ROOT)).toBeNull();
  });

  it("skips directory links", () => {
    expect(resolveFileHref("src/", ROOT, ROOT)).toBeNull();
    expect(resolveFileHref("src", ROOT, ROOT)?.path).toBe("/repo/src");
  });
});
