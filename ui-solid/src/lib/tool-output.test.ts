import { describe, expect, it } from "vitest";
import { parseToolOutput, summarizeToolOutput } from "./tool-output.ts";

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

describe("summarizeToolOutput", () => {
  it("previews the last line of successful output", () => {
    const summary = summarizeToolOutput(
      JSON.stringify({ metadata: { output: "Compiling\nFinished in 3.2s", exit: 0 } }),
      "completed",
    );
    expect(summary.preview).toBe("Finished in 3.2s");
    expect(summary.hiddenLineCount).toBe(1);
    expect(summary.exitCode).toBe(0);
  });

  it("previews the first error line for failures", () => {
    const summary = summarizeToolOutput(
      JSON.stringify({
        metadata: { output: "some output", exit: 1 },
        error: "error: missing dependency\nmore detail",
      }),
      "failed",
    );
    expect(summary.preview).toBe("error: missing dependency");
    expect(summary.hiddenLineCount).toBe(1);
  });

  it("treats a non-zero exit code as a failure", () => {
    const summary = summarizeToolOutput(
      JSON.stringify({ metadata: { output: "line one\nboom", exit: 2 } }),
      "completed",
    );
    expect(summary.preview).toBe("line one");
    expect(summary.exitCode).toBe(2);
  });

  it("treats a reported error as a failure", () => {
    const summary = summarizeToolOutput(
      JSON.stringify({ metadata: { output: "started\nmore" }, error: "boom\nstack" }),
      "completed",
    );
    expect(summary.preview).toBe("boom");
  });
});
