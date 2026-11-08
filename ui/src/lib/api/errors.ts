import type { ApiErrorResponse, ErrorCode, ErrorInfo } from "#lib/generated/bindings.ts";

/**
 * Structured API error common to REST / WS (`{ error: ErrorInfo }`).
 */
export class ApiError extends Error {
  /** HTTP status (0 when originating from WS). */
  readonly status: number;
  /** Structured error body (null when it cannot be parsed). */
  readonly info: ErrorInfo | null;

  constructor(status: number, info: ErrorInfo | null, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.info = info;
  }

  /** Structured error code (null when it cannot be parsed). */
  get code(): ErrorCode | null {
    return this.info?.code ?? null;
  }

  get isUnauthorized(): boolean {
    return this.status === 401 || this.code === "UNAUTHORIZED";
  }

  /** Build from an error response body (keeps the raw text when it cannot be parsed). */
  static fromBody(status: number, body: unknown, fallback: string): ApiError {
    const parsed = asApiErrorResponse(body);
    if (parsed) {
      return new ApiError(status, parsed.error, parsed.error.message);
    }
    return new ApiError(status, null, fallback);
  }

  /** Build from a code when no body is available (for example, a disconnected WS). */
  static fromCode(status: number, code: ErrorCode, message: string): ApiError {
    return new ApiError(status, { code, message }, message);
  }
}

function asApiErrorResponse(value: unknown): ApiErrorResponse | null {
  if (typeof value !== "object" || value === null) return null;
  const error = (value as { error?: unknown }).error;
  if (typeof error !== "object" || error === null) return null;
  const body = error as Partial<ErrorInfo>;
  if (typeof body.code !== "string" || typeof body.message !== "string") return null;
  return { error: body as ErrorInfo };
}
