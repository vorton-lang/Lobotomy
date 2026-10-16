import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';

type Request = { id: number; text: string; streaming: boolean };
type Response = { id: number; html: string; repaired?: boolean; error?: string };

const postMessage = vi.fn<(response: Response) => void>();
const workerScope = {
  postMessage,
  onmessage: null as ((event: MessageEvent<Request>) => Promise<void>) | null,
};
let nextId = 0;

beforeAll(async () => {
  vi.stubGlobal('self', workerScope);
  await import('./worker');
});

afterAll(() => vi.unstubAllGlobals());

async function respond(text: string, streaming: boolean): Promise<Response> {
  postMessage.mockClear();
  const id = ++nextId;
  await workerScope.onmessage!({ data: { id, text, streaming } } as MessageEvent<Request>);
  expect(postMessage).toHaveBeenCalledTimes(1);
  const response = postMessage.mock.calls[0][0];
  expect(response.id).toBe(id);
  expect(response.error).toBeUndefined();
  return response;
}

async function render(text: string, streaming: boolean): Promise<string> {
  return (await respond(text, streaming)).html;
}

describe('Markdown worker', () => {
  it('says whether live text needed a repair', async () => {
    expect((await respond('**open', true)).repaired).toBe(true);
    expect((await respond('done **ok**', true)).repaired).toBe(false);
  });

  it.each([
    ['an unclosed mark in an earlier paragraph', '*note: run the tests first\n\nsecond\n\nthird'],
    ['a backtick after a blank line inside an open fence', '```\na\n\n`b'],
    ['a $$ that is not math', 'The PID is $$ here'],
    ['single tildes', '20~25 and 30~40'],
    ['a list item starting with >', '- > 30'],
  ])('leaves %s as the completed text reads', async (_, text) => {
    expect(await render(text, true)).toBe(await render(text, false));
  });

  it('repairs the last paragraph only', async () => {
    expect(await render('*a\n\n**b', true)).toBe('<p>*a</p>\n<p><strong>b</strong></p>\n');
  });

  // The desktop app opens these without running programs (frontend.md §7.2).
  it('links local files by their absolute path', async () => {
    const link = async (target: string) => /href="([^"]*)"/.exec(await render(`[对照](${target})`, false))?.[1];
    expect(await link('</C:/Users/Yufeng Ying/p2/index.html>')).toBe('file:///C:/Users/Yufeng%20Ying/p2/index.html');
    expect(await link('<C:\\Users\\Yufeng Ying\\p2\\index.html>')).toBe('file:///C:/Users/Yufeng%20Ying/p2/index.html');
    expect(await link('file:///C:/a%20b/x.png')).toBe('file:///C:/a%20b/x.png');
    expect(await link('/home/u/report.pdf')).toBe('file:///home/u/report.pdf');
    expect(await render('[对照](</C:/x/index.html>)', false)).toContain('title="/C:/x/index.html"');
  });
});
