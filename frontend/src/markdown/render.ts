// The main thread's side of Markdown rendering: one worker, an LRU cache of finished results.

import { useEffect, useState } from 'react';
import { escapeHtml } from './escape';

/** Larger texts are shown as plain text; parsing them would stall the worker for long. */
const MARKDOWN_LIMIT = 200_000;
const CACHE_SIZE = 500;

let worker: Worker | null = null;
let nextId = 1;
const waiting = new Map<number, (html: string) => void>();
const cache = new Map<string, string>();

function getWorker() {
  if (!worker) {
    worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
    worker.onmessage = (event: MessageEvent<{ id: number; html: string }>) => {
      waiting.get(event.data.id)?.(event.data.html);
      waiting.delete(event.data.id);
    };
  }
  return worker;
}

// Identical text has different meaning while live and after completion.
const cacheKey = (text: string, streaming: boolean) => `${streaming ? 1 : 0}:${text}`;

export function renderMarkdown(text: string, streaming = false): Promise<string> {
  const key = cacheKey(text, streaming);
  const cached = cache.get(key);
  if (cached !== undefined) {
    cache.delete(key);
    cache.set(key, cached);
    return Promise.resolve(cached);
  }
  if (text.length > MARKDOWN_LIMIT) return Promise.resolve(`<pre>${escapeHtml(text)}</pre>`);
  const id = nextId++;
  return new Promise((resolve) => {
    waiting.set(id, (html) => {
      cache.set(key, html);
      if (cache.size > CACHE_SIZE) cache.delete(cache.keys().next().value!);
      resolve(html);
    });
    getWorker().postMessage({ id, text, streaming });
  });
}

/** Keep the previous live frame while rendering a delta, but never carry a repair into completion. */
export function useMarkdown(text: string, streaming = false): string | null {
  const [rendered, setRendered] = useState(() => ({ html: cache.get(cacheKey(text, streaming)) ?? null, streaming }));
  useEffect(() => {
    let current = true;
    void renderMarkdown(text, streaming).then((html) => current && setRendered({ html, streaming }));
    return () => {
      current = false;
    };
  }, [text, streaming]);
  return rendered.streaming === streaming ? rendered.html : cache.get(cacheKey(text, streaming)) ?? null;
}
