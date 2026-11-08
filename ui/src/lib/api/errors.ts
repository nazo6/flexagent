import type { ApiErrorBody, ApiErrorResponse, ErrorCode } from "#lib/generated/bindings.ts";

/**
 * Structured API error common to REST / WS (`{ error: { code, message } }`).
 */
export class ApiError extends Error {
  /** HTTP status (0 when originating from WS). */
  readonly status: number;
  /** Structured error code (null when it cannot be parsed). */
  readonly code: ErrorCode | null;

  constructor(status: number, code: ErrorCode | null, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }

  get isUnauthorized(): boolean {
    return this.status === 401 || this.code === "UNAUTHORIZED";
  }

  /** Build from an error response body (keeps the raw text when it cannot be parsed). */
  static fromBody(status: number, body: unknown, fallback: string): ApiError {
    const parsed = asApiErrorResponse(body);
    if (parsed) {
      return new ApiError(status, parsed.error.code, parsed.error.message);
    }
    return new ApiError(status, null, fallback);
  }
}

function asApiErrorResponse(value: unknown): ApiErrorResponse | null {
  if (typeof value !== "object" || value === null) return null;
  const error = (value as { error?: unknown }).error;
  if (typeof error !== "object" || error === null) return null;
  const body = error as Partial<ApiErrorBody>;
  if (typeof body.code !== "string" || typeof body.message !== "string") return null;
  return { error: { code: body.code as ErrorCode, message: body.message } };
}
