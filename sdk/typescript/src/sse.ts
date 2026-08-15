import { type SdkEvent, parseSdkEvent } from "./types.js";

/** Pull complete `data:` payloads from an SSE buffer. Keepalives (`: ping`) are skipped. */
export function popSseData(buf: { text: string }): string[] {
  const out: string[] = [];
  buf.text = buf.text.replace(/\r\n/g, "\n");
  for (;;) {
    const idx = buf.text.indexOf("\n\n");
    if (idx < 0) {
      break;
    }
    const frame = buf.text.slice(0, idx);
    buf.text = buf.text.slice(idx + 2);
    const lines: string[] = [];
    for (const line of frame.split("\n")) {
      if (line.startsWith("data:")) {
        lines.push(line.slice(5).replace(/^\s/, ""));
      }
    }
    if (lines.length > 0) {
      out.push(lines.join("\n"));
    }
  }
  return out;
}

export function decodeSsePayload(data: string): SdkEvent {
  try {
    return parseSdkEvent(JSON.parse(data) as unknown);
  } catch {
    return { ev: "unknown", raw: data };
  }
}

export async function* iterateSse(
  body: ReadableStream<Uint8Array> | null,
): AsyncGenerator<SdkEvent, void, void> {
  if (!body) {
    return;
  }
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const buf = { text: "" };
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (value) {
        buf.text += decoder.decode(value, { stream: !done });
      }
      for (const data of popSseData(buf)) {
        yield decodeSsePayload(data);
      }
      if (done) {
        break;
      }
    }
  } finally {
    reader.releaseLock();
  }
}
