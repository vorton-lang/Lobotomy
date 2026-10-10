// Exercise live pushes through the store, transcript components and the real Markdown worker.
import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';
import type { LiveTurn, Snapshot, ThreadPage } from '../src/api/types';

const origin = process.env.LOBOTOMY_TEST_URL ?? 'http://127.0.0.1:5173';

async function openStream(page: Page) {
  let socket!: WebSocketRoute;
  let live: Record<string, LiveTurn> = {};
  const thread: ThreadPage = { items: [], has_more: false, turns: {}, messages: [], commands: {}, queued: [] };
  await page.routeWebSocket(/\/gui\?token=markdown-test$/, (route) => {
    socket = route;
    route.onMessage((raw) => {
      const { id, method } = JSON.parse(String(raw));
      const snapshot: Snapshot = {
        seq: 1,
        project: { id: 'project', repo_path: '/project', branch: 'main', author_name: 'Test', author_email: 'test@example.com', integration: 'base', integration_rev: 1, previewed: 'base' },
        config: null, tasks: [], attention: [], quota: [], harnesses: [], live,
        roles: [{ name: 'Malkuth', kind: 'worker', harness: 'claude', model: null, slot: null, task_id: null, unfinished: null, last_turn: null, hold: null, stalled: null, outside: null, workspace: null, queued_messages: 0 }],
      };
      route.send(JSON.stringify({ id, result: method === 'snapshot' ? snapshot : thread }));
    });
  });
  await page.goto(`${origin}/?backend=ws://127.0.0.1:9999/gui&token=markdown-test`);
  await expect(page.getByText('已连接', { exact: true })).toBeVisible();
  return {
    update(text: string, second?: string) {
      const message = (text: string) => ({ kind: 'agent_message' as const, content: { text }, started_at: 1 });
      live = { turn: { role: 'Malkuth', started_at: 1, items: { reply: message(text), ...(second === undefined ? {} : { second: message(second) }) } } };
      socket.send(JSON.stringify({ type: 'live', live }));
    },
    complete(text?: string) {
      if (text !== undefined) {
        thread.items = [{ id: 'reply', turn_id: 'turn', seq: 1, command_id: null, created_at: 1, kind: 'agent_message', content: { text } }];
        socket.send(JSON.stringify({ type: 'thread', role: 'Malkuth' }));
      }
      live = {};
      socket.send(JSON.stringify({ type: 'live', live }));
    },
  };
}

for (const [label, chunks, selector, visible] of [
  ['bold', ['**wo', 'rld'], 'strong', 'world'],
  ['italic', ['*wo', 'rld'], 'em', 'world'],
  ['inline code', ['Use `con', 'st x'], 'code', 'const x'],
  ['link', ['[docs](https://exa', 'mple.com'], 'p', 'docs'],
] as const) {
  test(`streaming Markdown repairs incomplete ${label} only until item completion`, async ({ page }, testInfo) => {
    const api = await openStream(page);
    let text = '';
    for (const delta of chunks) {
      text += delta;
      api.update(text);
      const rendered = page.locator('.live .markdown:not(.plain)');
      await expect(rendered).toBeVisible();
      await expect(rendered).not.toContainText(label === 'link' ? 'https://' : text);
    }
    const live = page.locator('.live .markdown');
    await expect(live.locator(selector)).toHaveText(visible);
    if (label === 'link') await expect(live.locator('a')).toHaveCount(0);
    if (label === 'bold') await page.screenshot({ path: testInfo.outputPath('markdown-streaming.png') });

    // The persisted item has exactly the same source, without any synthetic closing marker.
    api.complete(text);
    await expect(page.locator('.live')).toHaveCount(0);
    const completed = page.locator('.message.other .markdown:not(.plain)');
    await expect(completed).toHaveText(text);
    if (label !== 'link') await expect(completed.locator(selector)).toHaveCount(0);
    else await expect(completed.locator('a')).toHaveAttribute('href', 'https://example.com/');
    if (label === 'bold') await page.screenshot({ path: testInfo.outputPath('markdown-completed.png') });
  });
}

test('streaming Markdown keeps simultaneous items separate and drops an interrupted live turn', async ({ page }) => {
  const api = await openStream(page);
  api.update('**first', '*second');
  await expect(page.locator('.live strong')).toHaveText('first');
  await expect(page.locator('.live em')).toHaveText('second');
  for (let n = 0; n < 30; n++) api.update(`**first ${n}`, `*second ${n}`);
  await expect(page.locator('.live strong')).toHaveText('first 29');
  await expect(page.locator('.live em')).toHaveText('second 29');
  // A failed/interrupted turn may end without item.completed. Its empty live push still removes repairs.
  api.complete();
  await expect(page.locator('.live')).toHaveCount(0);
  await expect(page.locator('.message.other')).toHaveCount(0);
});

// Hold real worker replies so completion and rapid prop changes can overtake older work.
test('streaming Markdown ignores stale worker replies and changes mode on the same mounted hook', async ({ page }) => {
  await page.addInitScript(() => {
    const pending: MessageEvent[] = [];
    let releasing = false;
    let worker: Worker;
    const NativeWorker = window.Worker;
    window.Worker = class extends NativeWorker {
      constructor(url: string | URL, options?: WorkerOptions) {
        super(url, options);
        worker = this;
        this.addEventListener('message', (event) => {
          if (!releasing) { pending.push(event); event.stopImmediatePropagation(); }
        });
      }
    };
    Object.assign(window, {
      markdownReplies: pending,
      releaseMarkdown(index: number) {
        const event = pending.splice(index, 1)[0];
        releasing = true;
        worker.dispatchEvent(new MessageEvent('message', { data: event.data }));
        releasing = false;
      },
    });
  });
  await page.goto(origin);
  await page.evaluate(async () => {
    const react = 'react';
    const dom = 'react-dom/client';
    const common = '/src/components/common.tsx';
    // Vite resolves bare imports in source modules, so use its already-served dependency URLs here.
    const { default: { createElement } } = await import(`/node_modules/.vite/deps/${react}.js`);
    const { default: { createRoot } } = await import(`/node_modules/.vite/deps/${dom.replace('/', '_')}.js`);
    const { Markdown } = await import(common);
    const host = document.createElement('section');
    host.id = 'markdown-probe';
    document.body.append(host);
    const root = createRoot(host);
    Object.assign(window, { showMarkdown: (text: string, streaming: boolean) => root.render(createElement(Markdown, { text, streaming })) });
  });
  const show = (text: string, streaming: boolean) => page.evaluate(({ text, streaming }) => (window as any).showMarkdown(text, streaming), { text, streaming });
  const pending = () => page.evaluate(() => (window as any).markdownReplies.length);
  const release = (index: number) => page.evaluate((n) => (window as any).releaseMarkdown(n), index);
  const rendered = page.locator('#markdown-probe .markdown');
  await show('**old', true);
  await expect.poll(pending).toBe(1);
  await show('**new', true);
  await expect.poll(pending).toBe(2);
  await release(1);
  await expect(rendered.locator('strong')).toHaveText('new');
  await release(0);
  await expect(rendered.locator('strong')).toHaveText('new');

  await show('**new', false);
  await expect(rendered).toHaveText('**new');
  await expect(rendered.locator('strong')).toHaveCount(0);
  await expect.poll(pending).toBe(1);
  await release(0);
  await expect(rendered).not.toHaveClass(/plain/);
  await expect(rendered).toHaveText('**new');
  // Both modes are cached separately even for one source string.
  await show('**new', true);
  await expect(rendered.locator('strong')).toHaveText('new');
  await show('**new', false);
  await expect(rendered.locator('strong')).toHaveCount(0);
  await expect(rendered).toHaveText('**new');
  expect(await pending()).toBe(0);

  await show('**late', true);
  await expect.poll(pending).toBe(1);
  await show('**late', false);
  await expect.poll(pending).toBe(2);
  await release(1);
  await expect(rendered).toHaveText('**late');
  await release(0);
  await expect(rendered.locator('strong')).toHaveCount(0);
  await expect(rendered).toHaveText('**late');
});
