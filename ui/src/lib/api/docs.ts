import type { ErrorCode } from "#lib/generated/bindings.ts";

/** Base URL of the troubleshooting guide (anchored per `ErrorCode`). */
const TROUBLESHOOTING_URL = "https://github.com/nazo6/flexagent/blob/main/docs/troubleshooting.md";

/** Link to the troubleshooting section for an error code (null when unknown). */
export function troubleshootingUrl(code: ErrorCode | null): string | null {
  if (code === null) return null;
  return `${TROUBLESHOOTING_URL}#${code.toLowerCase()}`;
}
