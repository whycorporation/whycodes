import { spawn, type ChildProcess } from "node:child_process";
import { createServer } from "node:net";
import { delimiter } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { existsSync } from "node:fs";

import { SdkError } from "./error.js";
import { iterateSse } from "./sse.js";
import {
  PROTOCOL_MAJOR,
  type Handshake,
  type LaunchOptions,
  type PermissionDecision,
  type RunOptions,
  type SdkEvent,
  type SessionInfo,
  type StructuredAttempt,
  type StructuredResult,
  type TurnResult,
  type UsageSnapshot,
  extractJson,
  parseHandshake,
  parseSessionInfo,
  validateInstance,
  validateSchema,
} from "./types.js";

export class WhycodeClient {
  readonly baseUrl: string;
  #child: ChildProcess | undefined;

  private constructor(baseUrl: string, child?: ChildProcess) {
    this.baseUrl = baseUrl;
    this.#child = child;
  }

  /** Attach to an already-running daemon (`whycode serve`). */
  static async connect(base: string): Promise<WhycodeClient> {
    const client = new WhycodeClient(normalizeBase(base));
    await client.health();
    return client;
  }

  /**
   * Spawn `whycode serve` as a private instance and connect to it.
   * The child inherits this process's environment (API keys, `HOME`).
   */
  static async launch(opts: LaunchOptions = {}): Promise<WhycodeClient> {
    const workingDir = opts.workingDir ?? process.cwd();
    const startupTimeoutMs = opts.startupTimeoutMs ?? 15_000;
    const port = opts.port ?? (await ephemeralPort());
    const binary = resolveBinary(opts.binary);
    const child = spawn(binary, ["serve", String(port)], {
      cwd: workingDir,
      stdio: ["ignore", "ignore", "pipe"],
      env: process.env,
    });
    const stderrChunks: Buffer[] = [];
    child.stderr?.on("data", (chunk: Buffer) => {
      stderrChunks.push(chunk);
    });

    const baseUrl = `http://127.0.0.1:${port}`;
    const client = new WhycodeClient(baseUrl, child);
    const deadline = Date.now() + startupTimeoutMs;

    try {
      for (;;) {
        if (Date.now() >= deadline) {
          const stderr = Buffer.concat(stderrChunks).toString("utf8").trim();
          throw new SdkError(
            "startup_timeout",
            `daemon at ${baseUrl} did not become healthy in ${startupTimeoutMs}ms.${stderr ? ` stderr: ${stderr}` : ""}`,
          );
        }
        const exit = child.exitCode;
        if (exit !== null) {
          const stderr = Buffer.concat(stderrChunks).toString("utf8").trim();
          throw new SdkError(
            "startup_failed",
            `whycode serve exited (${exit}).${stderr ? ` stderr: ${stderr}` : ""}`,
          );
        }
        try {
          await client.health();
          return client;
        } catch (err) {
          if (err instanceof SdkError && err.code === "unsupported_version") {
            throw err;
          }
          await delay(50);
        }
      }
    } catch (err) {
      await client.close();
      throw err;
    }
  }

  async health(): Promise<Handshake> {
    const res = await this.#request("GET", "/v1/health");
    if (res.status === 404) {
      throw new SdkError(
        "unsupported_version",
        `${this.baseUrl} has no /v1/health — upgrade whycode serve (need protocol ${PROTOCOL_MAJOR})`,
      );
    }
    if (!res.ok) {
      throw SdkError.fromStatus(res.status, "health");
    }
    const hs = parseHandshake(await res.json());
    if (!hs) {
      throw new SdkError("internal", "invalid handshake body");
    }
    if (hs.protocol !== PROTOCOL_MAJOR) {
      throw new SdkError(
        "unsupported_version",
        `daemon speaks protocol ${hs.protocol}, client speaks ${PROTOCOL_MAJOR}`,
      );
    }
    return hs;
  }

  async listSessions(): Promise<SessionInfo[]> {
    const res = await this.#request("GET", "/v1/sessions");
    if (!res.ok) {
      throw SdkError.fromStatus(res.status, "list sessions");
    }
    const body = (await res.json()) as { sessions?: unknown };
    const sessions: SessionInfo[] = [];
    if (Array.isArray(body.sessions)) {
      for (const row of body.sessions) {
        const info = parseSessionInfo(row);
        if (info) sessions.push(info);
      }
    }
    return sessions;
  }

  async createSession(project?: string): Promise<SessionInfo> {
    const payload: { persist: boolean; project?: string } = { persist: true };
    if (project !== undefined) {
      payload.project = project;
    }
    const res = await this.#request("POST", "/v1/sessions", payload);
    if (!res.ok) {
      throw SdkError.fromStatus(res.status, "create session");
    }
    const info = parseSessionInfo(await res.json());
    if (!info) {
      throw new SdkError("internal", "create session: missing id");
    }
    return info;
  }

  async getSession(id: string): Promise<SessionInfo> {
    const res = await this.#request("GET", `/v1/sessions/${encodeURIComponent(id)}`);
    if (res.status === 404) {
      throw new SdkError("unknown_session", `session ${id} not found`);
    }
    if (!res.ok) {
      throw SdkError.fromStatus(res.status, "get session");
    }
    const info = parseSessionInfo(await res.json());
    if (!info) {
      throw new SdkError("internal", "get session: invalid body");
    }
    return info;
  }

  /** Run one turn and collect the result. Use {@link runEvents} for a live UI.
   *  Defaults `autoApprove` to true so scripts do not hang on Ask. */
  async run(sessionId: string, message: string, opts: RunOptions = {}): Promise<TurnResult> {
    const collected: RunOptions = { ...opts, autoApprove: opts.autoApprove ?? true };
    return this.collectTurn(sessionId, message, collected);
  }

  private async collectTurn(
    sessionId: string,
    message: string,
    opts: RunOptions,
  ): Promise<TurnResult> {
    let text = "";
    const toolCalls: TurnResult["tool_calls"] = [];
    const toolNames = new Map<string, string>();
    let usage: UsageSnapshot | undefined;
    let cancelled = false;
    let lastError: SdkError | undefined;

    for await (const ev of this.runEvents(sessionId, message, opts)) {
      switch (ev.ev) {
        case "text_delta":
          text += ev.text;
          break;
        case "tool_start":
          toolNames.set(ev.id, ev.name);
          break;
        case "tool_end":
          toolCalls.push({
            id: ev.id,
            name: toolNames.get(ev.id) ?? "",
            is_error: ev.is_error,
          });
          break;
        case "usage":
          usage = {
            input_tokens: ev.input_tokens,
            output_tokens: ev.output_tokens,
            cache_read_input_tokens: ev.cache_read_input_tokens,
            cache_creation_input_tokens: ev.cache_creation_input_tokens,
          };
          break;
        case "cancelled":
          cancelled = true;
          break;
        case "turn_done":
          if (!text && ev.text) text = ev.text;
          break;
        case "error":
          lastError = new SdkError(ev.code, ev.message);
          break;
        default:
          break;
      }
    }

    if (lastError) {
      throw lastError;
    }
    const result: TurnResult = { text, tool_calls: toolCalls, cancelled };
    if (usage) result.usage = usage;
    return result;
  }

  async *runEvents(
    sessionId: string,
    message: string,
    opts: RunOptions = {},
  ): AsyncGenerator<SdkEvent, void, void> {
    const body: {
      message: string;
      provider?: string;
      model?: string;
      max_turns?: number;
      auto_approve?: boolean;
    } = { message };
    if (opts.provider !== undefined) body.provider = opts.provider;
    if (opts.model !== undefined) body.model = opts.model;
    if (opts.maxTurns !== undefined) body.max_turns = opts.maxTurns;
    if (opts.autoApprove !== undefined) body.auto_approve = opts.autoApprove;

    const res = await this.#request(
      "POST",
      `/v1/sessions/${encodeURIComponent(sessionId)}/run`,
      body,
    );
    if (res.status === 404) {
      throw new SdkError("unknown_session", `session ${sessionId} not found`);
    }
    if (res.status === 400) {
      throw new SdkError("invalid_request", "empty message");
    }
    if (!res.ok) {
      throw SdkError.fromStatus(res.status, "run");
    }
    for await (const ev of iterateSse(res.body)) {
      yield ev;
      if (ev.ev === "turn_done") {
        return;
      }
    }
  }

  async respondToPermission(
    sessionId: string,
    requestId: string,
    decision: PermissionDecision,
  ): Promise<void> {
    const res = await this.#request(
      "POST",
      `/v1/sessions/${encodeURIComponent(sessionId)}/permission`,
      { request_id: requestId, decision },
    );
    if (res.status === 404) {
      throw new SdkError("unknown_session", "unknown permission request");
    }
    if (!res.ok) {
      throw SdkError.fromStatus(res.status, "permission");
    }
  }

  /** Retry turns until the reply is JSON that matches `schema`. */
  async runStructured(
    sessionId: string,
    message: string,
    schema: unknown,
    opts: RunOptions = {},
    maxRetries = 2,
  ): Promise<StructuredResult> {
    const schemaErr = validateSchema(schema);
    if (schemaErr) {
      throw new SdkError("structured_schema_invalid", schemaErr);
    }
    const schemaTxt = JSON.stringify(schema, null, 2);
    let prompt = `${message}\n\nReply with a single JSON value that validates against this schema. No markdown, no commentary.\n${schemaTxt}`;
    const attempts: StructuredAttempt[] = [];
    for (let i = 0; i <= maxRetries; i++) {
      const turn = await this.run(sessionId, prompt, opts);
      try {
        const data = extractJson(turn.text);
        const errors = validateInstance(schema, data);
        const ok = errors.length === 0;
        attempts.push({ text: turn.text, ok, errors });
        if (ok) {
          return { data, attempts };
        }
        if (i === maxRetries) {
          throw new SdkError("structured_output_invalid", errors.join("; "));
        }
        prompt = `Your previous reply was not valid JSON for the schema.\nErrors:\n- ${errors.join("\n- ")}\nReply again with only the JSON value.`;
      } catch (err) {
        if (err instanceof SdkError) throw err;
        const e = err instanceof Error ? err.message : String(err);
        attempts.push({ text: turn.text, ok: false, errors: [e] });
        if (i === maxRetries) {
          throw new SdkError("structured_output_invalid", e);
        }
        prompt = `Your previous reply was not parseable JSON (${e}). Reply again with only the JSON value matching the schema.`;
      }
    }
    throw new SdkError("structured_output_invalid", "exhausted structured retries");
  }

  async cancel(sessionId: string): Promise<void> {
    const res = await this.#request(
      "POST",
      `/v1/sessions/${encodeURIComponent(sessionId)}/cancel`,
    );
    if (res.status === 404) {
      throw new SdkError("unknown_session", `no in-flight run for ${sessionId}`);
    }
    if (!res.ok) {
      throw SdkError.fromStatus(res.status, "cancel");
    }
  }

  /** Stop a launched child. No-op for {@link connect}. */
  async close(): Promise<void> {
    const child = this.#child;
    this.#child = undefined;
    if (!child || child.exitCode !== null) {
      return;
    }
    child.kill("SIGTERM");
    await Promise.race([
      new Promise<void>((resolve) => {
        child.once("exit", () => resolve());
      }),
      delay(2000),
    ]);
    if (child.exitCode === null) {
      child.kill("SIGKILL");
    }
  }

  async #request(method: string, path: string, json?: unknown): Promise<Response> {
    const init: RequestInit = {
      method,
      signal: AbortSignal.timeout(600_000),
    };
    if (json !== undefined) {
      init.headers = { "content-type": "application/json" };
      init.body = JSON.stringify(json);
    }
    try {
      return await fetch(`${this.baseUrl}${path}`, init);
    } catch (err) {
      throw SdkError.fromUnknown(err, "disconnected");
    }
  }
}

export function normalizeBase(addr: string): string {
  const t = addr.trim().replace(/\/+$/, "");
  if (t.startsWith("http://") || t.startsWith("https://")) {
    return t;
  }
  return `http://${t}`;
}

function resolveBinary(explicit?: string): string {
  if (explicit) return explicit;
  if (process.env.WHYCODE) return process.env.WHYCODE;
  const sibling = process.platform === "win32" ? "whycode.exe" : "whycode";
  if (existsSync(sibling)) return sibling;
  return onPath("whycode") ?? "whycode";
}

function onPath(name: string): string | undefined {
  const paths = (process.env.PATH ?? "").split(delimiter);
  const ext = process.platform === "win32" ? [".exe", ".cmd", ""] : [""];
  for (const dir of paths) {
    for (const e of ext) {
      const candidate = `${dir}${dir.endsWith("/") || dir.endsWith("\\") ? "" : "/"}${name}${e}`;
      if (existsSync(candidate)) return candidate;
    }
  }
  return undefined;
}

function ephemeralPort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      server.close((err) => {
        if (err) {
          reject(err);
          return;
        }
        if (addr && typeof addr === "object") {
          resolve(addr.port);
          return;
        }
        reject(new Error("ephemeral bind returned no port"));
      });
    });
  });
}
