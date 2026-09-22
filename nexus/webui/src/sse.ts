/**
 * Line-delimited JSON streaming (the contract's "SSE" bus emits JSON lines;
 * some endpoints may frame with `data:` — both are handled). Fetch-stream
 * based so we can send it away and reconnect with backoff.
 *
 * Both pumps go through the API-base override + bearer token (see api.ts).
 */
import { authHeaders, withBase } from './api';
import type { BusEvent, TaskLogLine } from './types';

function handleLine(line: string): Record<string, unknown> | null {
  let l = line.trim();
  if (!l || l.startsWith(':')) return null; // heartbeat/comment
  if (l.startsWith('data:')) l = l.slice(5).trim();
  if (!l.startsWith('{')) return null;
  try {
    const parsed: unknown = JSON.parse(l);
    return parsed && typeof parsed === 'object' ? parsed as Record<string, unknown> : null;
  } catch {
    return null;
  }
}

async function pump(
  res: Response,
  onLine: (obj: Record<string, unknown>) => void,
): Promise<void> {
  const reader = res.body?.getReader();
  if (!reader) throw new Error('stream has no body');
  const dec = new TextDecoder();
  let buf = '';
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    let idx = buf.indexOf('\n');
    while (idx >= 0) {
      const line = buf.slice(0, idx).replace(/\r$/, '');
      buf = buf.slice(idx + 1);
      const obj = handleLine(line);
      if (obj) onLine(obj);
      idx = buf.indexOf('\n');
    }
  }
  // flush tail
  const tail = handleLine(buf);
  if (tail) onLine(tail);
}

/** Auto-reconnecting event bus stream (GET /api/events). */
export class EventLineStream {
  private ctl: AbortController | null = null;
  private stopped = true;
  private backoffMs = 800;
  private readonly backoffMaxMs = 8000;

  constructor(
    private readonly path: string,
    private readonly onEvent: (ev: BusEvent) => void,
    private readonly onStatus?: (connected: boolean) => void,
  ) { /* — */ }

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    void this.loop();
  }

  stop(): void {
    this.stopped = true;
    this.ctl?.abort();
    this.ctl = null;
  }

  private async loop(): Promise<void> {
    while (!this.stopped) {
      this.ctl = new AbortController();
      const ctl = this.ctl;
      try {
        const res = await fetch(withBase(this.path), {
          cache: 'no-store', signal: ctl.signal, headers: authHeaders(),
        });
        if (!res.ok) throw new Error(`stream ${res.status}`);
        this.onStatus?.(true);
        this.backoffMs = 800;
        await pump(res, (obj) => this.onEvent(obj as BusEvent));
        this.onStatus?.(false);
      } catch (e) {
        this.onStatus?.(false);
        if (this.stopped || (e instanceof Error && e.name === 'AbortError')) return;
      }
      if (this.stopped) return;
      await new Promise((r) => window.setTimeout(r, this.backoffMs));
      this.backoffMs = Math.min(this.backoffMs * 2, this.backoffMaxMs);
    }
  }
}

/**
 * One-shot task log stream (GET /api/tasks/{id}/stream). Resolves when the
 * server closes the stream or `shouldStop()` returns true.
 */
export async function streamTaskLogs(
  taskId: string,
  onLine: (line: TaskLogLine) => void,
  signal?: AbortSignal,
): Promise<void> {
  let res: Response;
  try {
    res = await fetch(withBase(`/api/tasks/${encodeURIComponent(taskId)}/stream`), {
      cache: 'no-store',
      signal,
      headers: authHeaders(),
    });
  } catch (e) {
    if (e instanceof Error && e.name === 'AbortError') return;
    throw e;
  }
  if (!res.ok) throw new Error(`task stream ${res.status}`);
  await pump(res, (obj) => onLine(obj as unknown as TaskLogLine));
}
