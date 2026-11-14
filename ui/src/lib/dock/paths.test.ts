import { describe, expect, it } from "vitest";
import { basename, isAbsolutePath, joinPath } from "./paths.ts";

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
});
