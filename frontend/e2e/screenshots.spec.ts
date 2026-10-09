// Not a test: captures the main states for review. Run with `npx playwright test screenshots`.

import { expect, open as connect, taskPanel, test, type Backend } from './fixtures';
import type { Page } from '@playwright/test';

const out = process.env.SCREENSHOT_DIR ?? 'test-results/screenshots';

async function open(page: Page, backend: Backend) {
  await page.setViewportSize({ width: 1400, height: 860 });
  await connect(page, backend);
}

// A Codex that does not allow full access, and the settings (harness-adapter.md §1.9).
test('screenshots of a managed Codex', async ({ page, backend }) => {
  test.skip(!process.env.SCREENSHOT_DIR, 'review only');
  await open(page, backend);
  await page.getByRole('button', { name: '+ 新任务' }).click();
  const dialog = page.getByRole('dialog', { name: '新任务' });
  await dialog.getByLabel('标题').fill('受管环境的任务');
  await dialog.getByLabel('你的原话').fill('FAKE:managed FAKE:done FAKE:nowrite');
  await dialog.getByRole('button', { name: '交给 Malkuth' }).click();
  const card = page.locator('.attention .card').filter({ hasText: '没能启动' });
  await expect(card).toBeVisible();
  await page.screenshot({ path: `${out}/9-permission-refused.png` });
  await card.getByRole('button', { name: '改用自动审批并继续' }).click();
  const accept = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await expect(accept).toBeVisible();
  await page.getByRole('button', { name: '设置', exact: true }).click();
  const settings = page.getByRole('dialog', { name: '设置' });
  await expect(settings.getByRole('radio', { name: /^自动审批/ })).toBeChecked();
  await page.screenshot({ path: `${out}/10-settings.png` });
  await settings.getByRole('radio', { name: /^完全放开/ }).click();
  await expect(settings.getByRole('radio', { name: /^完全放开/ })).toBeChecked();
  await settings.getByRole('button', { name: '关闭' }).click();
  await accept.getByRole('button', { name: '查看并验收' }).click();
  const panel = taskPanel(page, '受管环境的任务');
  await panel.getByRole('button', { name: '验收', exact: true }).click();
  await expect(panel.locator('.phase')).toHaveText('已完成');
});

test('screenshots', async ({ page, backend }) => {
  test.skip(!process.env.SCREENSHOT_DIR, 'review only');
  await open(page, backend);
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
  const panel = taskPanel(page, '写 work.txt');
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
