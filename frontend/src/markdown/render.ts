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

export function renderMarkdown(text: string): Promise<string> {
  const cached = cache.get(text);
  if (cached !== undefined) {
    cache.delete(text);
    cache.set(text, cached);
    return Promise.resolve(cached);
  }
  if (text.length > MARKDOWN_LIMIT) return Promise.resolve(`<pre>${escapeHtml(text)}</pre>`);
  const id = nextId++;
  return new Promise((resolve) => {
    waiting.set(id, (html) => {
      cache.set(text, html);
      if (cache.size > CACHE_SIZE) cache.delete(cache.keys().next().value!);
      resolve(html);
    });
    getWorker().postMessage({ id, text, streaming: false });
  });
}

/** The rendered HTML once ready; `null` until then. */
export function useMarkdown(text: string): string | null {
  const [html, setHtml] = useState<string | null>(() => cache.get(text) ?? null);
  useEffect(() => {
    let current = true;
    void renderMarkdown(text).then((h) => current && setHtml(h));
    return () => {
      current = false;
    };
  }, [text]);
  return html;
}
