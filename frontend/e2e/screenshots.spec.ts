// Not a test: captures the main states for review. Run with `npx playwright test screenshots`.

import { expect, test } from '@playwright/test';
import fs from 'node:fs';
import { infoFile } from './setup';

const out = process.env.SCREENSHOT_DIR ?? 'test-results/screenshots';

test('screenshots', async ({ page }) => {
  test.skip(!process.env.SCREENSHOT_DIR, 'review only');
  const { port, token } = JSON.parse(fs.readFileSync(infoFile, 'utf8'));
  await page.setViewportSize({ width: 1400, height: 860 });
  await page.goto(`http://127.0.0.1:5173/?backend=${encodeURIComponent(`ws://127.0.0.1:${port}/gui`)}&token=${encodeURIComponent(token)}`);
  await expect(page.getByText('已连接')).toBeVisible();
  for (const [title, body] of [
    ['写 work.txt', 'FAKE:done'],
    ['会失败的任务', 'FAKE:fail'],
  ]) {
    await page.getByRole('button', { name: '+ 新任务' }).click();
    const dialog = page.getByRole('dialog', { name: '新任务' });
    await dialog.getByLabel('标题').fill(title);
    await dialog.getByLabel('你的原话').fill(body);
    await dialog.getByLabel('完成条件').fill('work.txt 存在，内容为 hi');
    if (title === '写 work.txt') await page.screenshot({ path: `${out}/1-new-task.png` });
    await dialog.getByRole('button', { name: '交给 Malkuth' }).click();
    if (title === '写 work.txt') await expect(page.locator('.attention .card').filter({ hasText: '等你验收' })).toBeVisible();
  }
  await page.screenshot({ path: `${out}/2-main.png` });
  await page.locator('.attention .card').filter({ hasText: '等你验收' }).getByRole('button', { name: '查看并验收' }).click();
  await expect(page.getByText('1 个文件')).toBeVisible();
  await page.waitForTimeout(1500);
  await page.screenshot({ path: `${out}/3-task-panel.png` });
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${out}/4-dark.png` });
  await page.emulateMedia({ colorScheme: 'light' });

  // A send-back: the panel tells the rounds apart, the history reads in time order.
  const panel = page.getByRole('dialog', { name: '写 work.txt' });
  await panel.getByRole('button', { name: '退回…' }).click();
  const sendBack = page.getByRole('dialog', { name: '退回候选成果' });
  await sendBack.getByLabel(/理由/).fill('内容改成 second。FAKE:done FAKE:text=second');
  await sendBack.getByRole('button', { name: '退回', exact: true }).click();
  await expect(panel.locator('.now')).toContainText('第 2 轮');
  await page.screenshot({ path: `${out}/5-sent-back.png` });
  await expect(panel.locator('.now')).toContainText('等你验收');
  await page.waitForTimeout(1500);
  await panel.locator('.panel-body').evaluate((e) => e.scrollTo(0, e.scrollHeight));
  await page.screenshot({ path: `${out}/6-round-2.png` });
  await panel.getByRole('button', { name: '关闭' }).click();
  await page.screenshot({ path: `${out}/7-thread.png` });

  await page.keyboard.press('Control+f');
  await page.getByLabel('搜索对话').fill('work.txt');
  await expect(page.getByRole('search').locator('.search-count')).toContainText('/');
  await page.getByLabel('搜索对话').press('Enter');
  await page.waitForTimeout(500);
  await page.screenshot({ path: `${out}/8-search.png` });
});
