// The main thread's side of Markdown rendering: one worker, an LRU cache of finished results.

import { useEffect, useState } from 'react';
import { escapeHtml } from './escape';

/** Larger texts are shown as plain text; parsing them would stall the worker for long. */
const MARKDOWN_LIMIT = 200_000;
const CACHE_SIZE = 500;

type Rendered = { html: string; repaired?: boolean };

let worker: Worker | null = null;
let nextId = 1;
const waiting = new Map<number, (rendered: Rendered) => void>();
/** `shared`: a live result that needed no repair, so it is also right while the text is live. */
const cache = new Map<string, { html: string; shared: boolean }>();

function getWorker() {
  if (!worker) {
    worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
    worker.onmessage = (event: MessageEvent<Rendered & { id: number }>) => {
      waiting.get(event.data.id)?.(event.data);
      waiting.delete(event.data.id);
    };
  }
  return worker;
}

// Identical text has different meaning while live and after completion. Live text that needed
// no repair renders exactly as the completed item will, so it is stored as the completed result:
// the item then shows it at once when it completes, instead of its source for one round trip.
const cacheKey = (text: string, streaming: boolean) => `${streaming ? 1 : 0}:${text}`;

function cached(text: string, streaming: boolean): string | undefined {
  const live = streaming ? cache.get(cacheKey(text, true)) : undefined;
  const completed = cache.get(cacheKey(text, false));
  const [key, entry] = live
    ? [cacheKey(text, true), live]
    : completed && (!streaming || completed.shared)
      ? [cacheKey(text, false), completed]
      : [];
  if (key === undefined || entry === undefined) return undefined;
  cache.delete(key);
  cache.set(key, entry);
  return entry.html;
}

export function renderMarkdown(text: string, streaming = false): Promise<string> {
  const hit = cached(text, streaming);
  if (hit !== undefined) return Promise.resolve(hit);
  if (text.length > MARKDOWN_LIMIT) return Promise.resolve(`<pre>${escapeHtml(text)}</pre>`);
  const id = nextId++;
  return new Promise((resolve) => {
    waiting.set(id, ({ html, repaired }) => {
      const shared = streaming && repaired === false;
      cache.set(cacheKey(text, streaming && !shared), { html, shared });
      if (cache.size > CACHE_SIZE) cache.delete(cache.keys().next().value!);
      resolve(html);
    });
    getWorker().postMessage({ id, text, streaming });
  });
}

/** Keep the previous live frame while rendering a delta, but never carry a repair into completion. */
export function useMarkdown(text: string, streaming = false): string | null {
  const [rendered, setRendered] = useState(() => ({ html: cached(text, streaming) ?? null, streaming }));
  useEffect(() => {
    let current = true;
    void renderMarkdown(text, streaming).then((html) => current && setRendered({ html, streaming }));
    return () => {
      current = false;
    };
  }, [text, streaming]);
  return rendered.streaming === streaming ? rendered.html : cached(text, streaming) ?? null;
}
