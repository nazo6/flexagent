import { describe, expect, it } from "vitest";
import { snippetSegments } from "./search.ts";

describe("snippetSegments", () => {
  it("returns the whole snippet as plain text when there is no marker", () => {
    expect(snippetSegments("no match here")).toEqual([{ text: "no match here", match: false }]);
  });

  it("splits a bracketed term from its context", () => {
    expect(snippetSegments("before [term] after")).toEqual([
      { text: "before ", match: false },
      { text: "term", match: true },
      { text: " after", match: false },
    ]);
  });

  it("handles a match at the start and several matches", () => {
    expect(snippetSegments("[one] and [two]")).toEqual([
      { text: "one", match: true },
      { text: " and ", match: false },
      { text: "two", match: true },
    ]);
  });

  it("treats an unterminated bracket as plain text", () => {
    expect(snippetSegments("a [broken")).toEqual([{ text: "a [broken", match: false }]);
  });

  it("returns no segments for an empty snippet", () => {
    expect(snippetSegments("")).toEqual([]);
  });
});
