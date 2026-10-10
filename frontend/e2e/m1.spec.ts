// The M1 chain through the GUI (roadmap.md M1): create a task, watch Malkuth work, look at the
// candidate's diff, accept it, and find the work in the user's repository. Each test has a backend
// of its own (fixtures.ts).

import fs from 'node:fs';
import path from 'node:path';
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

test('a task goes from creation to the user repository', async ({ page, backend }) => {
  await open(page, backend);
  // Pressing twice creates one task (#16).
  await createTask(page, '写 work.txt', 'FAKE:done', { twice: true });
  await expect(page.locator('.task-line', { hasText: '写 work.txt' })).toHaveCount(1);

  // Malkuth's report shows in the thread; the turn's end shows too; the candidate waits for
  // acceptance.
  await expect(page.locator('.report.done')).toContainText('写了 work.txt');
  await expect(page.locator('.turn-divider').first()).toContainText('turn 正常结束');
  const card = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await expect(card).toBeVisible();
  await card.getByRole('button', { name: '查看并验收' }).click();

  const panel = taskPanel(page, '写 work.txt');
  await expect(panel.locator('.badge.passed')).toHaveText('通过');
  await expect(panel.getByRole('region', { name: '交付说明' })).toContainText('写了 work.txt');
  await expect(panel.getByRole('heading', { name: '本轮交付（第 1 轮）' })).toBeVisible();
  await expect(panel.locator('.verification-details')).not.toHaveAttribute('open');
  await expect(panel.locator('.changes-details')).not.toHaveAttribute('open');
  await panel.locator('.changes-details > summary').click();
  await expect(panel.getByText('1 个文件')).toBeVisible();
  await panel.getByRole('button', { name: '验收', exact: true }).click();
  await expect(panel.locator('.phase')).toHaveText('已完成');

  const file = path.join(backend.repo, 'work.txt');
  await expect.poll(() => fs.existsSync(file), { timeout: 20_000 }).toBe(true);
  expect(fs.readFileSync(file, 'utf8')).toBe('hi');
  await expect(page.getByText('现在没有需要你处理的事。')).toBeVisible();
});

// The panel stays open from one candidate to the next: what it shows is always the candidate the
// "验收" button would accept (#13).
test('an open task panel follows the candidate through a send-back', async ({ page, backend }) => {
  await open(page, backend);
  // The test releases the check after inspecting the candidate. The file is outside the
  // repository and verification site, so materializing another round does not move it.
  const gate = path.join(path.dirname(backend.repo), 'check-gate');
  fs.writeFileSync(gate, '');
  await page.getByRole('button', { name: '设置', exact: true }).click();
  const settings = page.getByRole('dialog', { name: '设置' });
  await settings.getByRole('button', { name: '+ 添加检查命令' }).click();
  await settings.getByLabel('检查命令').fill(
    'node -e "const fs=require(\'fs\');setTimeout(()=>process.exit(1),30000);setInterval(()=>{if(fs.existsSync(process.env.LOBOTOMY_TEST_CHECK_GATE))process.exit(0)},20)"',
  );
  await settings.getByRole('button', { name: /保存项目设置为/ }).click();
  await expect(settings).toBeHidden();

  await createTask(page, '改 work.txt', 'FAKE:done FAKE:text=first');
  const card = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await card.getByRole('button', { name: '查看并验收' }).click();
  const panel = taskPanel(page, '改 work.txt');
  await panel.locator('.changes-details > summary').click();
  await expect(panel.locator('.diff')).toContainText('first');

  fs.unlinkSync(gate);
  await panel.getByRole('button', { name: '退回…' }).click();
  const sendBack = page.getByRole('dialog', { name: '退回候选成果' });
  await sendBack.getByLabel(/理由/).fill('FAKE:done FAKE:text=second');
  await sendBack.getByRole('button', { name: '退回', exact: true }).click();
  // While the new candidate is verified, its own changes show, not the last round's. The check
  // stays blocked until these assertions finish or its 30-second deadline expires.
  await expect(panel.locator('.now')).toHaveText('第 2 轮的候选成果正在验证。');
  await expect(panel.getByRole('heading', { name: '本轮交付（第 2 轮）' })).toBeVisible();
  await expect(panel.locator('.diff')).toContainText('second', { timeout: 2_000 });
  await expect(panel.locator('.diff')).not.toContainText('first');
  fs.writeFileSync(gate, '');
  await expect(panel.locator('.now')).toHaveText('第 2 轮的候选成果通过了验证，等你验收。');
  await expect(panel.locator('.diff')).toContainText('second');
  await expect(panel.locator('.diff')).not.toContainText('first');
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

// Exercise report-only states without making the fake CLI promise extra delivery formats.
// The real backend supplies the task; subsequent detail snapshots model report/capture timing.
test('delivery descriptions distinguish pending, older and invalidated rounds', async ({ page, backend }, testInfo) => {
  await page.setViewportSize({ width: 1400, height: 860 });
  await open(page, backend);
  await createTask(page, '查看交付内容', 'FAKE:done');
  await page.locator('.attention .card').filter({ hasText: '等你验收' }).getByRole('button', { name: '查看并验收' }).click();
  const panel = taskPanel(page, '查看交付内容');
  await expect(panel.getByRole('heading', { name: '本轮交付（第 1 轮）' })).toBeVisible();
  const delivery = panel.getByRole('region', { name: '交付说明' });
  const update = async (state: 'pending' | 'old' | 'new' | 'invalidated' | 'failed') => page.evaluate(async (state) => {
    const module = '/src/store.ts';
    const { useStore } = await import(module);
    const detail = structuredClone(useStore.getState().taskDetail);
    const first = detail.attempts[0];
    first.done_summary = '预览入口：[示例页面](https://example.com)\n\n本地文件：`dist/report.html`\n\n体验前运行 `npm run dev`，重点检查页面内容。';
    if (state === 'pending') first.candidate_id = null;
    else first.candidate_id = 'candidate1';
    detail.attempts = state === 'old' || state === 'new' || state === 'invalidated'
      ? [first, { ...first, id: 'second', seq: 2, candidate_id: null, done_summary: state === 'new' ? '第二轮成果：`output.txt`' : null }]
      : [first];
    if (state === 'failed') detail.verifications.at(-1).state = 'failed';
    useStore.setState({ taskDetail: detail });
  }, state);

  await update('pending');
  await expect(delivery).toContainText('成果尚未固定');
  await expect(delivery.getByRole('link', { name: '示例页面' })).toHaveAttribute('href', 'https://example.com/');
  await expect(delivery.locator('code', { hasText: 'npm run dev' })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('delivery-desktop.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(delivery).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('delivery-mobile.png') });
  await update('old');
  await expect(delivery).toContainText('上一轮交付说明（第 1 轮）');
  await expect(delivery).toContainText('本轮还没有交付说明');
  await update('new');
  await expect(delivery).toContainText('本轮交付（第 2 轮）');
  await expect(delivery).toContainText('第二轮成果');
  await expect(delivery).not.toContainText('示例页面');
  await update('invalidated');
  await expect(delivery).not.toContainText('第二轮成果');
  await expect(delivery).toContainText('上一轮交付说明');
  await update('failed');
  await expect(panel.locator('.verification-details')).toHaveAttribute('open', '');
  await expect(panel.locator('.verification-details > summary .badge')).toHaveText('未通过');
});
