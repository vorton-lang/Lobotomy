import { afterEach, expect, test, vi } from 'vitest';

const sent: { id: number; text: string; streaming: boolean }[] = [];
let reply: (event: { data: { id: number; html: string } }) => void;
vi.stubGlobal('Worker', class {
  set onmessage(callback: typeof reply) { reply = callback; }
  postMessage(message: typeof sent[number]) { sent.push(message); }
});
const { renderMarkdown } = await import('./render');
afterEach(() => { sent.length = 0; });

function finish(index: number, html: string) { reply({ data: { id: sent[index].id, html } }); }

test('live and completed results have separate cache entries for the same source', async () => {
  const text = '**same';
  const live = renderMarkdown(text, true);
  const final = renderMarkdown(text);
  expect(sent.map(({ streaming }) => streaming)).toEqual([true, false]);
  finish(1, '<p>**same</p>');
  finish(0, '<p><strong>same</strong></p>');
  expect(await live).toBe('<p><strong>same</strong></p>');
  expect(await final).toBe('<p>**same</p>');
  expect(await renderMarkdown(text, true)).toBe(await live);
  expect(await renderMarkdown(text, false)).toBe(await final);
  expect(sent).toHaveLength(2);
});

test('out-of-order requests resolve only their own result', async () => {
  const first = renderMarkdown('*first', true);
  const second = renderMarkdown('`second', true);
  finish(1, '<code>second</code>');
  finish(0, '<em>first</em>');
  expect(await first).toBe('<em>first</em>');
  expect(await second).toBe('<code>second</code>');
});

test('large live and completed input stays escaped plain text without starting worker jobs', async () => {
  const text = '<img src=x>' + '*'.repeat(200_000);
  for (const streaming of [true, false]) {
    expect(await renderMarkdown(text, streaming)).toBe(`<pre>&lt;img src=x&gt;${'*'.repeat(200_000)}</pre>`);
  }
  expect(sent).toHaveLength(0);
});
