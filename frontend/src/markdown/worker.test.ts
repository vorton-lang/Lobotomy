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
  it.each([
    ['**world', '<strong>world</strong>', '**world'],
    ['*world', '<em>world</em>', '*world'],
    ['Use `const x', 'Use <code>const x</code>', 'Use `const x'],
  ])('repairs %s only while streaming', async (text, live, complete) => {
    expect(await render(text, true)).toBe(`<p>${live}</p>\n`);
    expect(await render(text, false)).toBe(`<p>${complete}</p>\n`);
  });

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

  it('hides an unfinished link destination only while streaming', async () => {
    const text = '[docs](https://example.com';
    expect(await render(text, true)).toBe('<p>docs</p>\n');
    const completed = await render(text, false);
    expect(completed).toContain('[docs](');
    expect(completed).toContain('<a href="https://example.com/"');
    expect(completed).toContain('>https://example.com</a>');
  });

  it.each([true, false])('preserves complete links with streaming=%s', async (streaming) => {
    expect(await render('[docs](https://example.com)', streaming)).toBe(
      '<p><a href="https://example.com/" target="_blank" rel="noreferrer">docs</a></p>\n',
    );
  });

  it.each([true, false])('does not activate unsafe links with streaming=%s', async (streaming) => {
    const html = await render('[script](javascript:alert%281%29) [local](file:///etc/passwd)', streaming);
    expect(html).toBe('<p>script local</p>\n');
    expect(html).not.toContain('<a');
  });

  it.each([true, false])('escapes HTML and omits images with streaming=%s', async (streaming) => {
    const html = await render('<script>alert(1)</script>\n\n![avatar](https://example.com/avatar.png)', streaming);
    expect(html).toContain('&lt;script&gt;alert(1)&lt;/script&gt;');
    expect(html).toContain('<p>avatar</p>');
    expect(html).not.toContain('<script');
    expect(html).not.toContain('<img');
    expect(html).not.toContain('src=');
  });

  it.each([true, false])('uses escaped fallback for an unknown code language with streaming=%s', async (streaming) => {
    expect(await render('```not-a-real-language\n<script> & text\n```', streaming)).toBe(
      '<pre class="code"><code>&lt;script&gt; &amp; text</code></pre>',
    );
  });

  it('uses escaped fallback for code longer than the highlight limit', async () => {
    const code = `${'x'.repeat(50_000)}<&`;
    expect(await render(`\`\`\`javascript\n${code}\n\`\`\``, true)).toBe(
      `<pre class="code"><code>${'x'.repeat(50_000)}&lt;&amp;</code></pre>`,
    );
  });

  it.each([true, false])('currently highlights both open and closed fences with streaming=%s', async (streaming) => {
    const open = await render('```javascript\nconst answer = 42;', streaming);
    const closed = await render('```javascript\nconst answer = 42;\n```', streaming);
    expect(open).toContain('class="shiki');
    expect(open).toContain('answer');
    expect(open).toBe(closed);
  });
});
