/** Negotiated major version. Refuse the handshake if the daemon disagrees. */
export const PROTOCOL_MAJOR = 1 as const;

export const ERROR_CODES = [
  "disconnected",
  "timeout",
  "unknown_session",
  "invalid_request",
  "auth",
  "internal",
  "serve_not_found",
  "startup_failed",
  "startup_timeout",
  "unsupported_version",
  "cancelled",
] as const;

export type ErrorCode = (typeof ERROR_CODES)[number];

export function isErrorCode(value: string): value is ErrorCode {
  return (ERROR_CODES as readonly string[]).includes(value);
}

/** Wire `ev` values the client understands. Anything else becomes `unknown`. */
export const KNOWN_EVS = [
  "text_delta",
  "reasoning_delta",
  "tool_start",
  "tool_end",
  "usage",
  "status",
  "cancelled",
  "turn_done",
  "error",
  "intent",
  "file_conflict",
  "swarm_status",
  "background",
] as const;

export type KnownEv = (typeof KNOWN_EVS)[number];

export type Handshake = {
  protocol: number;
  version: string;
  healthy: boolean;
  project: string;
  uptime_secs: number;
  sessions_in_memory: number;
};

export type SessionInfo = {
  id: string;
  title: string;
  project: string;
  messages?: number;
  updated_at?: string;
  source?: string;
};

export type SessionList = {
  sessions: SessionInfo[];
};

export type RunOptions = {
  provider?: string;
  model?: string;
  maxTurns?: number;
};

export type LaunchOptions = {
  workingDir?: string;
  port?: number;
  binary?: string;
  startupTimeoutMs?: number;
};

export type ToolCallSummary = {
  id: string;
  name: string;
  is_error: boolean;
};

export type UsageSnapshot = {
  input_tokens: number;
  output_tokens: number;
  cache_read_input_tokens: number;
  cache_creation_input_tokens: number;
};

export type TurnResult = {
  text: string;
  tool_calls: ToolCallSummary[];
  usage?: UsageSnapshot;
  cancelled: boolean;
};

export type SdkEvent =
  | { ev: "text_delta"; text: string }
  | { ev: "reasoning_delta"; text: string }
  | { ev: "tool_start"; id: string; name: string; input: unknown }
  | { ev: "tool_end"; id: string; content: string; is_error: boolean }
  | {
      ev: "usage";
      input_tokens: number;
      output_tokens: number;
      cache_read_input_tokens: number;
      cache_creation_input_tokens: number;
    }
  | { ev: "status"; message: string }
  | { ev: "cancelled" }
  | { ev: "turn_done"; text: string }
  | { ev: "error"; code: ErrorCode; message: string }
  | {
      ev: "intent";
      kind: string;
      confidence: number;
      badge: string;
      notice_kind: string;
      notice: string;
    }
  | { ev: "file_conflict"; path: string; claimant: string; owner: string }
  | { ev: "swarm_status"; active: number; total: number; message: string }
  | { ev: "background"; id: string; status: string; summary: string }
  | { ev: "unknown"; raw: unknown };

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function str(obj: Record<string, unknown>, key: string, fallback = ""): string {
  const v = obj[key];
  return typeof v === "string" ? v : fallback;
}

function num(obj: Record<string, unknown>, key: string, fallback = 0): number {
  const v = obj[key];
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}

function bool(obj: Record<string, unknown>, key: string, fallback = false): boolean {
  const v = obj[key];
  return typeof v === "boolean" ? v : fallback;
}

/** Parse one SSE JSON payload into a typed event. Never throws. */
export function parseSdkEvent(value: unknown): SdkEvent {
  const obj = asRecord(value);
  if (!obj || typeof obj.ev !== "string") {
    return { ev: "unknown", raw: value };
  }
  switch (obj.ev) {
    case "text_delta":
      return { ev: "text_delta", text: str(obj, "text") };
    case "reasoning_delta":
      return { ev: "reasoning_delta", text: str(obj, "text") };
    case "tool_start":
      return {
        ev: "tool_start",
        id: str(obj, "id"),
        name: str(obj, "name"),
        input: obj.input ?? null,
      };
    case "tool_end":
      return {
        ev: "tool_end",
        id: str(obj, "id"),
        content: str(obj, "content"),
        is_error: bool(obj, "is_error"),
      };
    case "usage":
      return {
        ev: "usage",
        input_tokens: num(obj, "input_tokens"),
        output_tokens: num(obj, "output_tokens"),
        cache_read_input_tokens: num(obj, "cache_read_input_tokens"),
        cache_creation_input_tokens: num(obj, "cache_creation_input_tokens"),
      };
    case "status":
      return { ev: "status", message: str(obj, "message") };
    case "cancelled":
      return { ev: "cancelled" };
    case "turn_done":
      return { ev: "turn_done", text: str(obj, "text") };
    case "error": {
      const codeRaw = str(obj, "code", "internal");
      return {
        ev: "error",
        code: isErrorCode(codeRaw) ? codeRaw : "internal",
        message: str(obj, "message"),
      };
    }
    case "intent":
      return {
        ev: "intent",
        kind: str(obj, "kind"),
        confidence: num(obj, "confidence"),
        badge: str(obj, "badge"),
        notice_kind: str(obj, "notice_kind"),
        notice: str(obj, "notice"),
      };
    case "file_conflict":
      return {
        ev: "file_conflict",
        path: str(obj, "path"),
        claimant: str(obj, "claimant"),
        owner: str(obj, "owner"),
      };
    case "swarm_status":
      return {
        ev: "swarm_status",
        active: num(obj, "active"),
        total: num(obj, "total"),
        message: str(obj, "message"),
      };
    case "background":
      return {
        ev: "background",
        id: str(obj, "id"),
        status: str(obj, "status"),
        summary: str(obj, "summary"),
      };
    default:
      return { ev: "unknown", raw: value };
  }
}

export function parseHandshake(value: unknown): Handshake | null {
  const obj = asRecord(value);
  if (!obj || typeof obj.protocol !== "number") {
    return null;
  }
  return {
    protocol: obj.protocol,
    version: str(obj, "version"),
    healthy: bool(obj, "healthy", true),
    project: str(obj, "project"),
    uptime_secs: num(obj, "uptime_secs"),
    sessions_in_memory: num(obj, "sessions_in_memory"),
  };
}

export function parseSessionInfo(value: unknown): SessionInfo | null {
  const obj = asRecord(value);
  if (!obj || typeof obj.id !== "string") {
    return null;
  }
  const info: SessionInfo = {
    id: obj.id,
    title: str(obj, "title"),
    project: str(obj, "project"),
  };
  if (typeof obj.messages === "number") info.messages = obj.messages;
  if (typeof obj.updated_at === "string") info.updated_at = obj.updated_at;
  if (typeof obj.source === "string") info.source = obj.source;
  return info;
}
