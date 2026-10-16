// The task chain through the GUI: create a task, watch Malkuth work, read its delivery and accept
// it. Each test has a backend of its own (fixtures.ts).

import { createTask, expect, open, taskPanel, test } from './fixtures';

// On a slow machine an older page arrives later than the next render. Scrolling up must still
// load page after page to the start (the baseline found it stuck at the top in CI).
// Run the longest independent scenario early so it does not leave one worker idle at the end.
test('scrolling up on a slow machine loads every older page', async ({ page, backend }) => {
  await open(page, backend);
  // Three hundred items: the thread opens three pages after the task's brief.
  await createTask(page, '很长的任务', 'FAKE:many=300');
  await expect(page.locator('.attention .card').filter({ hasText: '还没有完成' })).toBeVisible();
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 });
  await page.reload();
  await expect(page.getByText('已连接')).toBeVisible();
  // The socket opens before the thread loads; scroll the list, not its loading placeholder.
  await expect(page.locator('.thread-area .thread')).toBeVisible();
  const reached = await page.evaluate(
    (marker) =>
      new Promise<boolean>((resolve) => {
        const thread = document.querySelector('.thread') as HTMLElement;
        const start = performance.now();
        const step = () => {
          if (thread.textContent?.includes(marker)) resolve(true);
          else if (performance.now() - start > 60_000) resolve(false);
          else {
            thread.scrollTop -= 1500;
            requestAnimationFrame(step);
          }
        };
        step();
      }),
    '任务：很长的任务（第 1 轮执行）',
  );
  expect(reached).toBe(true);
});

test('a task goes from creation to acceptance', async ({ page, backend }) => {
  await open(page, backend);
  // Pressing twice creates one task (#16).
  await createTask(page, '写 work.txt', 'FAKE:done', { twice: true });
  await expect(page.locator('.task-line', { hasText: '写 work.txt' })).toHaveCount(1);

  // Malkuth's report shows in the thread; the turn's end shows too; the candidate waits for
  // acceptance.
  await expect(page.locator('.report.done')).toContainText('写了 work.txt');
  await expect(page.locator('.turn-divider.completed').first()).toHaveAttribute('title', /turn 正常结束/);
  const card = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await expect(card).toBeVisible();
  await card.getByRole('button', { name: '查看并验收' }).click();

  const panel = taskPanel(page, '写 work.txt');
  await expect(panel.getByRole('region', { name: '交付说明' })).toContainText('写了 work.txt');
  await expect(panel.getByRole('heading', { name: '本轮交付（第 1 轮）' })).toBeVisible();
  await panel.getByRole('button', { name: '验收', exact: true }).click();
  await expect(panel.locator('.phase')).toHaveText('已完成');
  await expect(page.getByText('现在没有需要你处理的事。')).toBeVisible();
});

// The panel stays open from one candidate to the next: what it shows is always the candidate the
// "验收" button would accept (#13).
test('an open task panel follows the candidate through a send-back', async ({ page, backend }) => {
  await open(page, backend);
  await createTask(page, '改 work.txt', 'FAKE:done FAKE:text=first');
  const card = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await card.getByRole('button', { name: '查看并验收' }).click();
  const panel = taskPanel(page, '改 work.txt');
  await expect(panel.getByRole('heading', { name: '本轮交付（第 1 轮）' })).toBeVisible();

  await panel.getByRole('button', { name: '退回…' }).click();
  const sendBack = page.getByRole('dialog', { name: '退回候选成果' });
  await sendBack.getByLabel(/理由/).fill('FAKE:done FAKE:text=second');
  await sendBack.getByRole('button', { name: '退回', exact: true }).click();
  await expect(panel.getByRole('heading', { name: '本轮交付（第 2 轮）' })).toBeVisible();
  await expect(panel.locator('.now')).toHaveText('第 2 轮交出了候选成果，等你验收。');
  await expect(panel.locator('.history')).toContainText('你退回了：FAKE:done FAKE:text=second');

  await panel.getByRole('button', { name: '验收', exact: true }).click();
  await expect(panel.locator('.phase')).toHaveText('已完成');
});

// A command that writes a file through a heredoc takes a line or two until opened; search still
// reaches its end (#14). Its whole text opens over the window and copies whole (#15).
test('a long command stays short until opened', async ({ page, backend }) => {
  await open(page, backend);
  const composer = page.getByLabel('消息');
  // Forty items before it make the thread taller than the window.
  await composer.fill('FAKE:many=40 FAKE:longcmd');
  await composer.press('Enter');
  const item = page.locator('.item.command', { hasText: 'cat > generated.txt' });
  await expect(item.locator('summary')).toContainText('共 405 行');
  expect((await item.locator('summary').boundingBox())!.height).toBeLessThan(60);

  await page.keyboard.press('Control+f');
  const bar = page.getByRole('search');
  await bar.getByLabel('搜索对话').fill('heredoc-end-marker');
  await expect(bar.locator('.search-count')).toHaveText('1 / 1');
  await expect(item).toHaveAttribute('open', '');
  await expect.poll(() => page.evaluate(() => CSS.highlights.get('search-current')?.size ?? 0)).toBeGreaterThan(0);
  await bar.getByLabel('搜索对话').press('Escape');

  // Opened from a row low in a long thread, the viewer is where the window shows it, top to
  // bottom: nothing of the thread covers or clips it.
  await page.locator('.thread').evaluate((thread) => thread.scrollTo(0, thread.scrollHeight));
  await item.getByRole('button', { name: '查看全文' }).click();
  const viewer = page.getByRole('dialog', { name: '全文' });
  await expect(viewer.locator('.cm-content')).toBeVisible();
  const shown = await viewer.evaluate((dialog) => {
    const r = dialog.getBoundingClientRect();
    const inside = (f: number) => {
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height * f);
      return hit !== null && dialog.contains(hit);
    };
    return r.top >= 0 && r.bottom <= innerHeight && r.height > 400 && [0.05, 0.5, 0.95].every(inside);
  });
  expect(shown).toBe(true);

  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  const clipboard = () => page.evaluate(() => navigator.clipboard.readText());
  // The command FAKE:longcmd emits. The Windows clipboard ends lines with CRLF.
  const body = Array.from({ length: 402 }, (_, i) => `  line ${i} of the generated source;`);
  const command = ["cat > generated.txt <<'EOF'", ...body, 'heredoc-end-marker', 'EOF'].join('\n');
  const whole = (text: string) => expect(text.replace(/\r\n/g, '\n')).toBe(command);
  await viewer.locator('.cm-content').click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.press('ControlOrMeta+c');
  whole(await clipboard());
  await page.evaluate(() => navigator.clipboard.writeText(''));
  await viewer.getByRole('button', { name: '复制全文' }).click();
  await expect.poll(clipboard).not.toBe('');
  whole(await clipboard());
  await viewer.getByRole('button', { name: '关闭' }).click();
});

// Ctrl+F finds what the virtualized thread has not loaded, loads it and scrolls there
// (frontend.md §4.1 "搜索").
test('search goes to a match pages back', async ({ page, backend }) => {
  await open(page, backend);
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
