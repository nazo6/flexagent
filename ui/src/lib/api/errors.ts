import type { ApiErrorBody } from "$lib/generated/ApiErrorBody";
import type { ApiErrorResponse } from "$lib/generated/ApiErrorResponse";
import type { ErrorCode } from "$lib/generated/ErrorCode";

/**
 * REST / WS 共通の構造化 API エラー (`{ error: { code, message } }`)。
 */
export class ApiError extends Error {
  /** HTTP ステータス (WS 由来の場合は 0)。 */
  readonly status: number;
  /** 構造化エラーコード (パース不能時は null)。 */
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

  /** エラーレスポンスボディから組み立てる (パース不能時は生テキストを保持)。 */
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
