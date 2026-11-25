import { describe, expect, it } from "vitest";
import { parseToolOutput } from "./tool-output.ts";

describe("parseToolOutput", () => {
  it("keeps plain text output", () => {
    expect(parseToolOutput("hello\nworld")).toEqual({
      text: "hello\nworld",
      exitCode: null,
      error: null,
      truncated: false,
    });
  });

  it("unwraps the metadata envelope used by OpenCode", () => {
    expect(
      parseToolOutput(
        JSON.stringify({ metadata: { output: "test result: ok\n", exit: 0, truncated: true } }),
      ),
    ).toEqual({
      text: "test result: ok\n",
      exitCode: 0,
      error: null,
      truncated: true,
    });
  });

  it("extracts the error and exit code of a failed call", () => {
    expect(
      parseToolOutput(
        JSON.stringify({
          metadata: { output: "compiling", exit: 1 },
          error: "exit status 1",
        }),
      ),
    ).toEqual({
      text: "compiling",
      exitCode: 1,
      error: "exit status 1",
      truncated: false,
    });
  });

  it("falls back to the raw string when the JSON is malformed", () => {
    expect(parseToolOutput("{not json")).toMatchObject({ text: "{not json", exitCode: null });
  });

  it("accepts an exit code encoded as a numeric string", () => {
    expect(parseToolOutput(JSON.stringify({ metadata: { exit: "2" } })).exitCode).toBe(2);
  });

  it("treats empty output as absent", () => {
    expect(parseToolOutput("  ")).toEqual({
      text: "",
      exitCode: null,
      error: null,
      truncated: false,
    });
  });
});
