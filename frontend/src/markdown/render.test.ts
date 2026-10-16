import { afterEach, expect, test, vi } from 'vitest';

const sent: { id: number; text: string; streaming: boolean }[] = [];
let reply: (event: { data: { id: number; html: string; repaired?: boolean } }) => void;
vi.stubGlobal(
  'Worker',
  class {
    set onmessage(callback: typeof reply) {
      reply = callback;
    }
    postMessage(message: (typeof sent)[number]) {
      sent.push(message);
    }
  },
);
const { renderMarkdown } = await import('./render');
afterEach(() => {
  sent.length = 0;
});

function finish(index: number, html: string, repaired?: boolean) {
  reply({ data: { id: sent[index].id, html, repaired } });
}

test('a live result that needed no repair also serves the completed item', async () => {
  const live = renderMarkdown('plain reply', true);
  finish(0, '<p>plain reply</p>', false);
  expect(await live).toBe('<p>plain reply</p>');
  expect(await renderMarkdown('plain reply', false)).toBe('<p>plain reply</p>');
  expect(await renderMarkdown('plain reply', true)).toBe('<p>plain reply</p>');
  expect(sent).toHaveLength(1);
});

test('a completed result never serves live text that needs a repair', async () => {
  const final = renderMarkdown('**open', false);
  finish(0, '<p>**open</p>');
  await final;
  const live = renderMarkdown('**open', true);
  expect(sent).toHaveLength(2);
  finish(1, '<p><strong>open</strong></p>', true);
  expect(await live).toBe('<p><strong>open</strong></p>');
  expect(await renderMarkdown('**open', false)).toBe('<p>**open</p>');
});
