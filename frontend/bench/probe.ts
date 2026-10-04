// What the baseline measures in the page (frontend.md §5): main-thread stalls (long tasks), input
// latency, frame intervals, the backend's response times as the page sees them, memory and DOM
// size. The app is not instrumented; this runs beside it as an init script and through the
// browser's DevTools protocol.
//
// Input latency is keydown to the start of the next frame: what the main thread adds. The
// browser's Event Timing, which runs to the frame's presentation, is not used: in the automated
// browser it reported several hundred ms whenever nothing else asked for frames, while a
// screencast of the same typing showed a frame for every key (notes/perf-baseline.md).

import type { CDPSession, Page } from '@playwright/test';

/** Installed before the app's own scripts. */
function install() {
  const bench = {
    longtasks: [] as { start: number; duration: number }[],
    /** keydown to the frame after it, in ms. */
    keys: [] as number[],
    frames: null as number[] | null,
    /** Request to response over the app's WebSocket, by method. */
    requests: [] as { method: string; at: number; ms: number }[],
  };
  (window as unknown as { __bench: typeof bench }).__bench = bench;
  new PerformanceObserver((list) => {
    for (const e of list.getEntries()) bench.longtasks.push({ start: e.startTime, duration: e.duration });
  }).observe({ type: 'longtask', buffered: true });
  addEventListener(
    'keydown',
    (event) => {
      const start = event.timeStamp;
      requestAnimationFrame(() => setTimeout(() => bench.keys.push(performance.now() - start), 0));
    },
    true,
  );
  const sent = new Map<number, { method: string; at: number }>();
  const send = WebSocket.prototype.send;
  WebSocket.prototype.send = function (data) {
    try {
      const { id, method } = JSON.parse(String(data));
      if (typeof id === 'number' && method) sent.set(id, { method, at: performance.now() });
    } catch {
      // Not a request.
    }
    return send.call(this, data);
  };
  const Native = WebSocket;
  window.WebSocket = class extends Native {
    constructor(url: string | URL, protocols?: string | string[]) {
      super(url, protocols);
      // Registered before the app's handler, so the time excludes the app's own handling.
      this.addEventListener('message', (event) => {
        // Keys are sorted: a result starts with its id, an error ends with it.
        const text = String(event.data);
        const id = (/^\{"id":(\d+)/.exec(text) ?? /"id":(\d+)\}$/.exec(text))?.[1];
        const request = id ? sent.get(Number(id)) : undefined;
        if (!request) return;
        sent.delete(Number(id));
        bench.requests.push({ method: request.method, at: request.at, ms: performance.now() - request.at });
      });
    }
  };
}

export interface Probe {
  page: Page;
  cdp: CDPSession;
}

export async function probe(page: Page): Promise<Probe> {
  await page.addInitScript(install);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Performance.enable');
  return { page, cdp };
}

export const round = (n: number) => Math.round(n * 10) / 10;

export function stats(values: number[]) {
  if (values.length === 0) return { count: 0 };
  const sorted = [...values].sort((a, b) => a - b);
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  return { count: values.length, p50: round(at(0.5)), p95: round(at(0.95)), max: round(sorted.at(-1)!) };
}

/** Long tasks since `since` (ms after navigation). */
export async function longTasks({ page }: Probe, since = 0) {
  const tasks = await page.evaluate(
    (since) => (window as unknown as { __bench: { longtasks: { start: number; duration: number }[] } }).__bench.longtasks.filter((t) => t.start >= since),
    since,
  );
  const durations = tasks.map((t) => t.duration);
  return { count: tasks.length, total_ms: round(durations.reduce((a, b) => a + b, 0)), max_ms: round(Math.max(0, ...durations)) };
}

/** Response times of one method since the last call for it. */
export async function takeRequests({ page }: Probe, method: string) {
  const times = await page.evaluate((method) => {
    const bench = (window as unknown as { __bench: { requests: { method: string; ms: number }[] } }).__bench;
    const mine = bench.requests.filter((r) => r.method === method).map((r) => r.ms);
    bench.requests = bench.requests.filter((r) => r.method !== method);
    return mine;
  }, method);
  return stats(times);
}

export async function now({ page }: Probe): Promise<number> {
  return page.evaluate(() => performance.now());
}

/** Resolves with `performance.now()` at the first frame where the selector's text includes `text`. */
export async function whenText({ page }: Probe, selector: string, text: string, timeoutMs = 60_000): Promise<number> {
  return page.evaluate(
    ({ selector, text, timeoutMs }) =>
      new Promise<number>((resolve, reject) => {
        const end = performance.now() + timeoutMs;
        const check = () => {
          const found = [...document.querySelectorAll(selector)].some((e) => e.textContent?.includes(text));
          if (found) resolve(performance.now());
          else if (performance.now() > end) reject(new Error(`no ${selector} with "${text}"`));
          else requestAnimationFrame(check);
        };
        check();
      }),
    { selector, text, timeoutMs },
  );
}

export async function startFrames({ page }: Probe) {
  await page.evaluate(() => {
    const bench = (window as unknown as { __bench: { frames: number[] | null } }).__bench;
    bench.frames = [];
    const tick = (t: number) => {
      if (!bench.frames) return;
      bench.frames.push(t);
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
}

/** Frame intervals since `startFrames`. */
export async function stopFrames({ page }: Probe) {
  const times = await page.evaluate(() => {
    const bench = (window as unknown as { __bench: { frames: number[] | null } }).__bench;
    const frames = bench.frames ?? [];
    bench.frames = null;
    return frames;
  });
  const intervals = times.slice(1).map((t, i) => t - times[i]);
  return { ...stats(intervals), over_50ms: intervals.filter((i) => i > 50).length };
}

/** Input latency of the keys typed since the last call: keydown to the start of the next frame. */
export async function takeKeys({ page }: Probe) {
  const keys = await page.evaluate(() => {
    const bench = (window as unknown as { __bench: { keys: number[] } }).__bench;
    const keys = bench.keys;
    bench.keys = [];
    return keys;
  });
  return stats(keys);
}

/** Heap after a garbage collection, and the document's size. */
export async function memory({ cdp }: Probe) {
  await cdp.send('HeapProfiler.collectGarbage');
  const { metrics } = await cdp.send('Performance.getMetrics');
  const metric = (name: string) => metrics.find((m) => m.name === name)?.value ?? 0;
  return { heap_mb: round(metric('JSHeapUsedSize') / 1024 / 1024), dom_nodes: metric('Nodes'), listeners: metric('JSEventListeners') };
}
