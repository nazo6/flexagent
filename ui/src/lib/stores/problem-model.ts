import { errorCodeMessage } from "#lib/api/error-messages.ts";
import { ApiError } from "#lib/api/errors.ts";
import type {
  DataSafety,
  ErrorCode,
  ErrorInfo,
  JsonValue,
  RecoveryAction,
} from "#lib/generated/bindings.ts";

/** Visual severity of a problem shown in the UI. */
export type ProblemSeverity = "error" | "warning";

/** A user-visible problem collected from any error boundary in the UI. */
export interface AppProblem {
  /** Unique ID (used for dismissal and keyed rendering). */
  id: string;
  /** Deduplication key (context + code + message). */
  key: string;
  severity: ProblemSeverity;
  /** Where the failure happened (`sessions`, `inbox`, `send prompt`, ...). */
  context: string;
  /** Localized message (falls back to the English server message). */
  message: string;
  code: ErrorCode | null;
  /** Technical details for the expandable section (null when absent). */
  details: string | null;
  /** Correlation ID shared with server logs / future incidents. */
  requestId: string | null;
  /** Troubleshooting documentation anchor. */
  docs: string | null;
  /** Recovery actions offered by the server. */
  recovery: RecoveryAction[];
  /** What happened to the user's data (null when not reported). */
  dataSafety: DataSafety | null;
  /** Creation time (epoch ms). */
  at: number;
}

/** Upper bound on retained problems (oldest are dropped). */
export const MAX_PROBLEMS = 20;

/** Message shown for an unknown thrown value. */
export const UNKNOWN_ERROR_MESSAGE = "Unexpected error";

/** Normalizes any thrown value into the fields shown in the banner. */
export function buildProblem(
  context: string,
  error: unknown,
  severity: ProblemSeverity,
  sequence: number,
): AppProblem {
  const normalized = normalizeError(error);
  return {
    id: `${Date.now()}-${sequence}`,
    key: `${context}\u0000${normalized.code ?? ""}\u0000${normalized.message}`,
    severity,
    context,
    ...normalized,
    at: Date.now(),
  };
}

/** Fields derived from an error, independent of context / severity. */
export interface NormalizedError {
  message: string;
  code: ErrorCode | null;
  details: string | null;
  requestId: string | null;
  docs: string | null;
  recovery: RecoveryAction[];
  dataSafety: DataSafety | null;
}

/** Normalizes `ApiError`, `Error`, strings and arbitrary values. */
export function normalizeError(error: unknown): NormalizedError {
  if (error instanceof ApiError) {
    const info = error.info;
    return {
      message:
        errorCodeMessage(info?.code ?? null) ??
        info?.message ??
        error.message ??
        UNKNOWN_ERROR_MESSAGE,
      code: info?.code ?? null,
      details: formatDetails(info?.details),
      requestId: info?.request_id ?? null,
      docs: info?.docs ?? null,
      recovery: info?.recovery ?? [],
      dataSafety: info?.data_safety ?? null,
    };
  }
  if (error instanceof Error) {
    return {
      message: error.message || UNKNOWN_ERROR_MESSAGE,
      code: null,
      details: error.stack ?? null,
      requestId: null,
      docs: null,
      recovery: [],
      dataSafety: null,
    };
  }
  if (typeof error === "string") {
    return {
      message: error,
      code: null,
      details: null,
      requestId: null,
      docs: null,
      recovery: [],
      dataSafety: null,
    };
  }
  return {
    message: UNKNOWN_ERROR_MESSAGE,
    code: null,
    details: formatDetails(error as JsonValue),
    requestId: null,
    docs: null,
    recovery: [],
    dataSafety: null,
  };
}

/** Reports a structured error body (`ErrorInfo`) that is not an exception. */
export function problemFromInfo(
  context: string,
  info: ErrorInfo,
  severity: ProblemSeverity,
  sequence: number,
): AppProblem {
  return buildProblem(context, new ApiError(0, info, info.message), severity, sequence);
}

function formatDetails(value: JsonValue | null | undefined): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}
