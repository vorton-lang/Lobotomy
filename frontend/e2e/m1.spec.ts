// The M1 chain through the GUI (roadmap.md M1): create a task, watch Malkuth work, look at the
// candidate's diff, accept it, and find the work in the user's repository.

import { expect, test, type Page } from '@playwright/test';
import fs from 'node:fs';
import path from 'node:path';
import { infoFile } from './setup';

const info = () => JSON.parse(fs.readFileSync(infoFile, 'utf8')) as { port: number; token: string; repo: string };

async function open(page: Page) {
  const { port, token } = info();
  await page.goto(`/?backend=${encodeURIComponent(`ws://127.0.0.1:${port}/gui`)}&token=${encodeURIComponent(token)}`);
  await expect(page.getByText('已连接')).toBeVisible();
}

async function createTask(page: Page, title: string, body: string) {
  await page.getByRole('button', { name: '+ 新任务' }).click();
  const dialog = page.getByRole('dialog', { name: '新任务' });
  await dialog.getByLabel('标题').fill(title);
  await dialog.getByLabel('你的原话').fill(body);
  await dialog.getByLabel('完成条件').fill('work.txt 存在');
  await dialog.getByRole('button', { name: '交给 Malkuth' }).click();
}

test.use({ baseURL: 'http://127.0.0.1:5173' });

test('a task goes from creation to the user repository', async ({ page }) => {
  await open(page);
  await createTask(page, '写 work.txt', 'FAKE:done');

  // Malkuth's report shows in the thread; the turn's end shows too; the candidate waits for
  // acceptance.
  await expect(page.locator('.report.done')).toContainText('写了 work.txt');
  await expect(page.locator('.turn-divider').first()).toContainText('turn 正常结束');
  const card = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await expect(card).toBeVisible();
  await card.getByRole('button', { name: '查看并验收' }).click();

  const panel = page.getByRole('dialog', { name: '写 work.txt' });
  await expect(panel.locator('.badge.passed')).toHaveText('通过');
  await expect(panel.getByText('1 个文件')).toBeVisible();
  await panel.getByRole('button', { name: '验收', exact: true }).click();
  await expect(panel.locator('.phase')).toHaveText('已完成');

  const file = path.join(info().repo, 'work.txt');
  await expect.poll(() => fs.existsSync(file), { timeout: 20_000 }).toBe(true);
  expect(fs.readFileSync(file, 'utf8')).toBe('hi');
  await expect(page.getByText('现在没有需要你处理的事。')).toBeVisible();
});

// The panel stays open from one candidate to the next: what it shows is always the candidate the
// "验收" button would accept (#13).
test('an open task panel follows the candidate through a send-back', async ({ page }) => {
  await open(page);
  // A slow check keeps each candidate in verification long enough to look at it there.
  await page.getByRole('button', { name: '项目设置' }).click();
  const settings = page.getByRole('dialog', { name: /项目设置/ });
  await settings.getByRole('button', { name: '+ 添加检查命令' }).click();
  await settings.getByLabel('检查命令').fill('node -e "setTimeout(() => {}, 6000)"');
  await settings.getByRole('button', { name: /保存为/ }).click();
  await expect(settings).toBeHidden();

  await createTask(page, '改 work.txt', 'FAKE:done FAKE:text=first');
  const card = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await card.getByRole('button', { name: '查看并验收' }).click();
  const panel = page.getByRole('dialog', { name: '改 work.txt' });
  await expect(panel.locator('.diff')).toContainText('first');

  await panel.getByRole('button', { name: '退回…' }).click();
  const sendBack = page.getByRole('dialog', { name: '退回候选成果' });
  await sendBack.getByLabel(/理由/).fill('FAKE:done FAKE:text=second');
  await sendBack.getByRole('button', { name: '退回', exact: true }).click();
  // While the new candidate is verified, its own changes show, not the last round's. The check
  // runs for 6 s; an old diff left on screen would still be there after 2.
  await expect(panel.locator('.now')).toHaveText('第 2 轮的候选成果正在验证。');
  await expect(panel.locator('.diff')).toContainText('second', { timeout: 2_000 });
  await expect(panel.locator('.diff')).not.toContainText('first');
  await expect(panel.locator('.now')).toHaveText('第 2 轮的候选成果通过了验证，等你验收。');
  await expect(panel.locator('.diff')).toContainText('second');
  await expect(panel.locator('.diff')).not.toContainText('first');
  await expect(panel.locator('.history')).toContainText('你退回了：FAKE:done FAKE:text=second');

  await panel.getByRole('button', { name: '验收', exact: true }).click();
  await expect(panel.locator('.phase')).toHaveText('已完成');
});

test('a failed turn waits for the user to continue', async ({ page }) => {
  await open(page);
  await createTask(page, '会失败的任务', 'FAKE:fail');
  const card = page.locator('.attention .card').filter({ hasText: '失败，它停下等你决定' });
  await expect(card).toBeVisible();
  await card.getByRole('button', { name: '继续' }).click();
  await expect(card).toBeHidden();
  // The continued turn opens with the runtime's note.
  await expect(page.locator('.message.runtime').last()).toContainText('上一个 turn 失败');

  // It ends normally without reporting anything: the task waits for the user (#13).
  const stalled = page.locator('.attention .card').filter({ hasText: '还没有完成' });
  await expect(stalled).toBeVisible();
  await stalled.getByRole('button', { name: '查看任务' }).click();
  const panel = page.getByRole('dialog', { name: '会失败的任务' });
  await expect(panel.locator('.now')).toHaveText('第 1 轮执行中，还没有交出候选成果。');
  await panel.getByRole('button', { name: '放弃' }).click();
  await expect(stalled).toBeHidden();
});

// Ctrl+F finds what the virtualized thread has not loaded, loads it and scrolls there
// (frontend.md §4.1 "搜索").
test('search goes to a match pages back', async ({ page }) => {
  await open(page);
  await createTask(page, '搜索目标任务', 'FAKE:many=250');
  // The turn is over: its brief is now more than a page above the newest item.
  await expect(page.locator('.attention .card').filter({ hasText: '还没有完成' })).toBeVisible();
  const brief = page.locator('.thread .row', { hasText: '任务：搜索目标任务（第 1 轮执行）' });
  await expect(brief).toHaveCount(0);

  await page.keyboard.press('Control+f');
  const bar = page.getByRole('search');
  await bar.getByLabel('搜索对话').fill('搜索目标任务');
  await expect(bar.locator('.search-count')).toHaveText('1 / 1');
  await expect(brief).toBeVisible();
  await expect.poll(() => page.evaluate(() => CSS.highlights.get('search-current')?.size ?? 0)).toBeGreaterThan(0);

  await bar.getByLabel('搜索对话').press('Escape');
  await expect(bar).toBeHidden();
  expect(await page.evaluate(() => CSS.highlights.has('search-current'))).toBe(false);
});
