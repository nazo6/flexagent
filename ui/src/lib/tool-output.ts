/**
 * Human-readable projection of `raw_output` for terminal (`execute`) tools.
 *
 * ACP agents may send structured metadata (for example
 * `{"metadata":{"output":"...","exit":0}}`) or plain text. The UI should show
 * the command output rather than the raw JSON envelope.
 */

export interface ToolOutputView {
  /** Captured output (empty when the agent did not report any). */
  text: string;
  /** Process exit code when the agent reported one. */
  exitCode: number | null;
  /** Error message when the tool call failed. */
  error: string | null;
  /** Whether the agent truncated the captured output. */
  truncated: boolean;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

function asString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function asNumber(value: unknown): number | null {
  if (typeof value === "number" && Number.isInteger(value)) return value;
  if (typeof value === "string" && /^-?\d+$/.test(value)) return Number(value);
  return null;
}

function pickString(record: Record<string, unknown>, keys: readonly string[]): string | null {
  for (const key of keys) {
    const value = asString(record[key]);
    if (value !== null) return value;
  }
  return null;
}

function pickNumber(record: Record<string, unknown>, keys: readonly string[]): number | null {
  for (const key of keys) {
    const value = asNumber(record[key]);
    if (value !== null) return value;
  }
  return null;
}

/** Parses a tool call's raw output into displayable text and metadata. */
export function parseToolOutput(rawOutput: string | null): ToolOutputView {
  if (rawOutput === null || rawOutput.trim() === "") {
    return { text: "", exitCode: null, error: null, truncated: false };
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(rawOutput);
  } catch {
    return { text: rawOutput, exitCode: null, error: null, truncated: false };
  }

  const root = asRecord(parsed);
  if (root === null) {
    return {
      text: typeof parsed === "string" ? parsed : rawOutput,
      exitCode: null,
      error: null,
      truncated: false,
    };
  }

  const metadata = asRecord(root.metadata) ?? root;
  return {
    text:
      pickString(metadata, ["output", "text", "stdout", "stderr"]) ??
      pickString(root, ["output", "text", "stdout", "stderr"]) ??
      "",
    exitCode: pickNumber(metadata, ["exit", "exitCode", "exit_code", "code"]),
    error: pickString(root, ["error"]) ?? pickString(metadata, ["error"]),
    truncated: metadata.truncated === true || root.truncated === true,
  };
}
