import { describe, expect, it } from "vitest";
import { mergeCursor } from "./cursor";

describe("mergeCursor", () => {
  it("adopts the first cursor when none is stored", () => {
    expect(mergeCursor(null, 3)).toEqual({ reset: false, cursor: 3 });
  });

  it("adopts the incoming cursor when it moves forward", () => {
    expect(mergeCursor(10, 12)).toEqual({ reset: false, cursor: 12 });
  });

  it("keeps the current cursor for duplicate batches", () => {
    expect(mergeCursor(10, 10)).toEqual({ reset: false, cursor: 10 });
  });

  it("detects a store reset when the cursor is rewound", () => {
    expect(mergeCursor(10, 2)).toEqual({ reset: true, cursor: 2 });
    expect(mergeCursor(10, 0)).toEqual({ reset: true, cursor: 0 });
  });
});
