import { afterEach, beforeEach } from "vitest";

/**
 * Fails a test file when Solid 2's dev diagnostics report a reactivity bug.
 *
 * Solid reports these on `console.warn` / `console.error`; a test that only
 * checks rendered output would otherwise pass while the component re-subscribes
 * to its own writes or reads a reactive value untracked.
 */

const DIAGNOSTIC =
  /\[(STRICT_READ_UNTRACKED|EFFECT_WRITES_OWN_SOURCE|UNCOMMITTED_INPUT_OVERWRITTEN|REACTIVE_WRITE_IN_OWNED_SCOPE|PENDING_ASYNC_UNTRACKED_READ)\]/;

let diagnostics: string[] = [];

let originalWarn: typeof console.warn;
let originalError: typeof console.error;

beforeEach(() => {
  diagnostics = [];
  originalWarn = console.warn;
  originalError = console.error;
  console.warn = (...args: unknown[]) => {
    const text = args.map(String).join(" ");
    if (DIAGNOSTIC.test(text)) diagnostics.push(text);
    originalWarn(...args);
  };
  console.error = (...args: unknown[]) => {
    const text = args.map(String).join(" ");
    if (DIAGNOSTIC.test(text)) diagnostics.push(text);
    originalError(...args);
  };
});

afterEach(() => {
  console.warn = originalWarn;
  console.error = originalError;
  if (diagnostics.length > 0) {
    throw new Error(
      `Solid dev diagnostics were reported:\n${diagnostics.map((entry) => `- ${entry}`).join("\n")}`,
    );
  }
});
