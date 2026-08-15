import { type ErrorCode, isErrorCode } from "./types.js";

/** SDK failure. Branch on {@link SdkError.code}, not the message. */
export class SdkError extends Error {
  readonly code: ErrorCode;

  constructor(code: ErrorCode, message: string, options?: { cause?: unknown }) {
    super(`${code}: ${message}`, options);
    this.name = "SdkError";
    this.code = code;
  }

  static fromStatus(status: number, what: string): SdkError {
    const code: ErrorCode =
      status === 404
        ? "unknown_session"
        : status === 400
          ? "invalid_request"
          : status === 401
            ? "auth"
            : "internal";
    return new SdkError(code, `${what} failed: ${status}`);
  }

  static fromUnknown(err: unknown, fallback: ErrorCode = "disconnected"): SdkError {
    if (err instanceof SdkError) {
      return err;
    }
    if (err instanceof Error && err.name === "TimeoutError") {
      return new SdkError("timeout", err.message, { cause: err });
    }
    const message = err instanceof Error ? err.message : String(err);
    return new SdkError(fallback, message, { cause: err });
  }

  static coerceCode(value: string): ErrorCode {
    return isErrorCode(value) ? value : "internal";
  }
}
