import { describe, expect, it } from "vitest";
import {
  basename,
  dirname,
  isAbsolutePath,
  isInsidePath,
  joinPath,
  normalizePath,
} from "./paths.ts";

describe("dock paths", () => {
  it("joins POSIX paths", () => {
    expect(joinPath("/repo", "src/main.ts")).toBe("/repo/src/main.ts");
    expect(joinPath("/repo/", "src/main.ts")).toBe("/repo/src/main.ts");
    expect(joinPath("/repo", "/repo/src/main.ts")).toBe("/repo/src/main.ts");
  });

  it("joins Windows paths with backslashes", () => {
    expect(joinPath("C:\\repo", "src/main.ts")).toBe("C:\\repo\\src\\main.ts");
    expect(joinPath("C:\\repo\\", "src/main.ts")).toBe("C:\\repo\\src\\main.ts");
  });

  it("takes the basename of either separator", () => {
    expect(basename("/repo/src/main.ts")).toBe("main.ts");
    expect(basename("C:\\repo\\src\\main.ts")).toBe("main.ts");
    expect(basename("main.ts")).toBe("main.ts");
  });

  it("recognizes absolute paths", () => {
    expect(isAbsolutePath("/repo")).toBe(true);
    expect(isAbsolutePath("C:\\repo")).toBe(true);
    expect(isAbsolutePath("\\\\server\\share")).toBe(true);
    expect(isAbsolutePath("src/main.ts")).toBe(false);
  });

  it("takes the directory of either separator", () => {
    expect(dirname("/repo/src/main.ts")).toBe("/repo/src");
    expect(dirname("/repo")).toBe("/");
    expect(dirname("/main.ts")).toBe("/");
    expect(dirname("main.ts")).toBe(".");
    expect(dirname("C:\\repo\\src\\main.ts")).toBe("C:\\repo\\src");
    expect(dirname("C:\\repo")).toBe("C:\\");
    expect(dirname("\\\\server\\share\\a.ts")).toBe("\\\\server\\share");
  });

  it("normalizes dot segments for POSIX and Windows paths", () => {
    expect(normalizePath("/repo/./src/../src/main.ts")).toBe("/repo/src/main.ts");
    expect(normalizePath("/repo//src///main.ts")).toBe("/repo/src/main.ts");
    expect(normalizePath("../src/main.ts")).toBe("../src/main.ts");
    expect(normalizePath("./src/main.ts")).toBe("src/main.ts");
    expect(normalizePath("C:\\repo\\.\\src\\..\\main.ts")).toBe("C:\\repo\\main.ts");
    expect(normalizePath("\\\\server\\share\\a\\..\\b.ts")).toBe("\\\\server\\share\\b.ts");
  });

  it("checks workspace containment", () => {
    expect(isInsidePath("/repo/src/main.ts", "/repo")).toBe(true);
    expect(isInsidePath("/repo", "/repo")).toBe(true);
    expect(isInsidePath("/repo-other/main.ts", "/repo")).toBe(false);
    expect(isInsidePath("/etc/passwd", "/repo")).toBe(false);
    expect(isInsidePath("C:\\Repo\\src\\Main.ts", "c:\\repo")).toBe(true);
  });
});
