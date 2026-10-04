// The performance baseline (frontend.md §5): fixed synthetic scenarios against the production
// build and the release backend. It records numbers; budgets come after the baseline (#6 §5).
// Results go to bench-results/; notes/perf-baseline.md keeps the reference run.

import { expect, test, type Page } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { FIRST_TASK, START_MARKER, infoFile, startBackend, waitForFile, type BenchInfo } from './setup';
import { longTasks, memory, now, probe, round, startFrames, stats, stopFrames, takeKeys, takeRequests, whenText, type Probe } from './probe';

const info = () => JSON.parse(fs.readFileSync(infoFile, 'utf8')) as BenchInfo;
const results: Record<string, unknown> = {};
let browserVersion = '';

test.describe.configure({ mode: 'serial' });

async function open(page: Page): Promise<Probe> {
  const p = await probe(page);
  const { port, token } = info();
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto(`/?backend=${encodeURIComponent(`ws://127.0.0.1:${port}/gui`)}&token=${encodeURIComponent(token)}`, { waitUntil: 'commit' });
  return p;
}

const NEWEST = 'bench newest';

test('opening a thread of 2000+ items', async ({ page, browser }) => {
  browserVersion = browser.version();
  const p = await open(page);
  const ready = await whenText(p, '.thread .row', NEWEST);
  results.open = {
    newest_visible_ms: round(ready),
    long_tasks: await longTasks(p),
    snapshot_ms: await takeRequests(p, 'snapshot'),
    thread_page_ms: await takeRequests(p, 'thread'),
    ...(await memory(p)),
  };

  await page.reload({ waitUntil: 'commit' });
  const again = await whenText(p, '.thread .row', NEWEST);
  results.reload = { newest_visible_ms: round(again), long_tasks: await longTasks(p) };

  // The unclosed Markdown renders as Markdown, not as plain text.
  await expect(page.locator('.markdown strong', { hasText: 'An unclosed reply' })).toBeVisible();
});

test('scrolling back to the start of the thread', async ({ page }) => {
  const p = await open(page);
  await whenText(p, '.thread .row', NEWEST);
  await takeRequests(p, 'thread');
  const before = await now(p);
  await startFrames(p);
  // Scrolls up as fast as frames come, loading older pages on the way, until the first turn shows.
  const elapsed = await page.evaluate(
    (marker) =>
      new Promise<number>((resolve, reject) => {
        const start = performance.now();
        const thread = document.querySelector('.thread') as HTMLElement;
        const step = () => {
          if ([...document.querySelectorAll('.thread .row')].some((r) => r.textContent?.includes(marker))) resolve(performance.now() - start);
          else if (performance.now() - start > 120_000) reject(new Error('the start of the thread did not show'));
          else {
            thread.scrollTop -= 1500;
            requestAnimationFrame(step);
          }
        };
        step();
      }),
    START_MARKER,
  );
  results.scroll_to_start = {
    elapsed_ms: round(elapsed),
    older_pages_ms: await takeRequests(p, 'thread'),
    frames: await stopFrames(p),
    long_tasks: await longTasks(p, before),
    ...(await memory(p)),
  };
});

test('searching for the start of the thread', async ({ page }) => {
  const p = await open(page);
  await whenText(p, '.thread .row', NEWEST);
  await takeRequests(p, 'thread');
  await page.keyboard.press('Control+f');
  const field = page.getByLabel('搜索对话');
  await field.click();
  const before = await now(p);
  // Only the first turn's brief has it: the search loads every older page on the way there.
  await page.keyboard.type(FIRST_TASK);
  const found = await whenText(p, '.thread .row', START_MARKER);
  results.search_to_start = {
    found_ms: round(found - before),
    search_ms: await takeRequests(p, 'search'),
    older_pages_ms: await takeRequests(p, 'thread'),
    long_tasks: await longTasks(p, before),
  };
});

test('typing while idle', async ({ page }) => {
  const p = await open(page);
  await whenText(p, '.thread .row', NEWEST);
  const composer = page.getByLabel('消息');
  await composer.click();
  await takeKeys(p);
  await page.keyboard.type('这是一条用来测量输入延迟的消息 measuring input latency while idle', { delay: 30 });
  await page.waitForTimeout(200);
  results.typing_idle = await takeKeys(p);
  await composer.fill('');
});

test('opening a 100,000-line output', async ({ page }) => {
  const p = await open(page);
  await whenText(p, '.thread .row', NEWEST);
  const item = page.locator('.item.command', { hasText: 'bench huge output' });
  await item.locator('summary').click();
  const before = await now(p);
  await item.getByRole('button', { name: '查看全文' }).click();
  const shown = await whenText(p, '.viewer .cm-content', 'line 0 ');
  await page.locator('.viewer .cm-content').click();
  const keyAt = await now(p);
  await page.keyboard.press('Control+End');
  const atEnd = await whenText(p, '.viewer .cm-content', 'line 99999 ');
  results.huge_output = {
    viewer_ms: round(shown - before),
    to_last_line_ms: round(atEnd - keyAt),
    long_tasks: await longTasks(p, before),
    ...(await memory(p)),
  };
});

test('a running command updating every 20 ms', async ({ page }) => {
  const p = await open(page);
  await whenText(p, '.thread .row', NEWEST);
  const composer = page.getByLabel('消息');
  await composer.fill('FAKE:stream=8 FAKE:marker=stream-done');
  await composer.press('Enter');
  await whenText(p, '.thread .row', 'bench stream', 30_000);
  const before = await now(p);
  await startFrames(p);
  // How far behind the command's output the screen is: each line carries the time it was written.
  const lag = page.evaluate(
    () =>
      new Promise<number[]>((resolve) => {
        const samples: number[] = [];
        let last = -1;
        const sample = () => {
          const live = [...document.querySelectorAll('.item.command')].find((e) => e.textContent?.includes('bench stream'));
          const ticks = [...(live?.textContent ?? '').matchAll(/tick (\d+) at (\d+)/g)];
          const newest = ticks.at(-1);
          if (newest && Number(newest[1]) !== last) {
            last = Number(newest[1]);
            samples.push(Date.now() - Number(newest[2]));
          }
          if (document.querySelector('.thread')?.textContent?.includes('bench stream-done')) resolve(samples);
          else requestAnimationFrame(sample);
        };
        sample();
      }),
  );
  await composer.click();
  await takeKeys(p);
  await page.keyboard.type('typing while the output streams in, 输入延迟', { delay: 30 });
  const typing = await takeKeys(p);
  const samples = await lag;
  results.streaming = {
    screen_lag_ms: stats(samples),
    frames: await stopFrames(p),
    long_tasks: await longTasks(p, before),
    typing,
  };
  await composer.fill('');
});

test('the backend restarts and the GUI catches up', async ({ page }) => {
  const p = await open(page);
  await whenText(p, '.thread .row', NEWEST);
  const bench = info();
  const backendFile = path.join(bench.root, 'project', 'backend.json');
  const { pid } = JSON.parse(fs.readFileSync(backendFile, 'utf8')) as { pid: number };
  process.kill(pid);
  await expect(page.locator('.status.closed, .status.connecting')).toBeVisible();
  fs.rmSync(backendFile, { force: true });
  const spawned = Date.now();
  startBackend(bench);
  await waitForFile(backendFile);
  const listening = Date.now();
  await expect(page.locator('.status.open')).toBeVisible({ timeout: 30_000 });
  const connected = Date.now();
  await whenText(p, '.thread .row', 'bench stream-done');
  results.reconnect = {
    backend_start_ms: listening - spawned,
    gui_connected_after_listening_ms: connected - listening,
    thread_back_ms: Date.now() - listening,
  };
});

test.afterAll(() => {
  const commit = execFileSync('git', ['rev-parse', '--short', 'HEAD'], { encoding: 'utf8' }).trim();
  const run = {
    at: new Date().toISOString(),
    commit,
    machine: { os: `${os.type()} ${os.release()}`, cpu: os.cpus()[0]?.model, cores: os.cpus().length, memory_gb: Math.round(os.totalmem() / 2 ** 30) },
    browser: browserVersion,
    items: info().items,
    results,
  };
  const dir = path.resolve('bench-results');
  fs.mkdirSync(dir, { recursive: true });
  const text = JSON.stringify(run, null, 2);
  fs.writeFileSync(path.join(dir, `${run.at.replace(/[:.]/g, '-')}.json`), text);
  fs.writeFileSync(path.join(dir, 'latest.json'), text);
  console.log(text);
});
