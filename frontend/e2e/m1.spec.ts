// The M1 chain through the GUI (roadmap.md M1): create a task, watch Malkuth work, look at the
// candidate's diff, accept it, and find the work in the user's repository. Each test has a backend
// of its own (fixtures.ts).

import fs from 'node:fs';
import path from 'node:path';
import { createTask, expect, open, taskPanel, test } from './fixtures';

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
  // A slow check keeps each candidate in verification long enough to look at it there.
  await page.getByRole('button', { name: '设置', exact: true }).click();
  const settings = page.getByRole('dialog', { name: '设置' });
  await settings.getByRole('button', { name: '+ 添加检查命令' }).click();
  await settings.getByLabel('检查命令').fill('node -e "setTimeout(() => {}, 6000)"');
  await settings.getByRole('button', { name: /保存项目设置为/ }).click();
  await expect(settings).toBeHidden();

  await createTask(page, '改 work.txt', 'FAKE:done FAKE:text=first');
  const card = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await card.getByRole('button', { name: '查看并验收' }).click();
  const panel = taskPanel(page, '改 work.txt');
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

test('a failed turn waits for the user to continue', async ({ page, backend }) => {
  await open(page, backend);
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
  const panel = taskPanel(page, '会失败的任务');
  await expect(panel.locator('.now')).toHaveText('第 1 轮执行中，还没有交出候选成果。');
  await panel.getByRole('button', { name: '放弃' }).click();
  await expect(stalled).toBeHidden();
});

// The executor asks the user. The reply takes the question off "等你决定" at once, and the question
// does not come back while the executor works on the reply (#17).
test('a question leaves the list once the user replies', async ({ page, backend }) => {
  await open(page, backend);
  await createTask(page, '要先问的任务', 'FAKE:blocked');
  const card = page.locator('.attention .card').filter({ hasText: '执行者在等你' });
  await expect(card).toContainText('空值怎么处理？');
  const roleLine = page.locator('.role-line');
  await expect(roleLine).toContainText('提出了问题，等你回答');

  await card.getByLabel('回复').fill('FAKE:wait=3000 FAKE:done');
  await card.getByRole('button', { name: '发送' }).click();
  await expect(card).toBeHidden();
  await expect(roleLine).toContainText('正在跑 turn');
  await expect(page.locator('.attention h3 .count')).toHaveCount(0);
  await expect(page.getByText('现在没有需要你处理的事。')).toBeVisible();
  await expect(page.locator('.attention .card').filter({ hasText: '等你验收' })).toBeVisible();
});

// Once the next task starts, the open panel of the accepted one says so and leads to it (#18).
test('a finished task’s panel leads to the task now in work', async ({ page, backend }) => {
  await open(page, backend);
  await createTask(page, '第一个任务', 'FAKE:done');
  await createTask(page, '第二个任务', 'FAKE:done FAKE:text=two');
  const card = page.locator('.attention .card').filter({ hasText: '「第一个任务」通过了验证' });
  await card.getByRole('button', { name: '查看并验收' }).click();
  const first = taskPanel(page, '第一个任务');
  await expect(first.locator('.now-running')).toHaveCount(0);
  await first.getByRole('button', { name: '验收', exact: true }).click();
  await expect(first.locator('.phase')).toHaveText('已完成');
  await expect(first.locator('.now-running')).toContainText('「第二个任务」');
  await first.getByRole('button', { name: '查看当前任务' }).click();
  await expect(taskPanel(page, '第二个任务')).toBeVisible();
  // Nothing runs in the backend's directories when the test ends.
  await expect(page.locator('.attention .card').filter({ hasText: '「第二个任务」通过了验证' })).toBeVisible();
});

// Reading back in the thread, the user stays where they are. A button says when something new
// came and goes back to the end (#18). The report the last message repeats starts folded.
test('back to the latest after reading back', async ({ page, backend }) => {
  await open(page, backend);
  const composer = page.getByLabel('消息');
  await composer.fill('FAKE:many=60');
  await composer.press('Enter');
  const latest = page.getByRole('button', { name: /回到最新/ });
  await expect(page.locator('.thread .message.other').last()).toContainText('finished');
  await expect(latest).toBeHidden();

  // Just after new rows come, the list still goes to the end each time a row's height changes,
  // such as when its Markdown is rendered (virtua's scrollToIndex, for about 150 ms). A scroll
  // made then is undone, so scroll again until the list stays where it was scrolled to.
  await expect(async () => {
    await page.locator('.thread').evaluate((thread) => thread.scrollTo(0, 0));
    await expect(latest).toHaveText('回到最新 ↓', { timeout: 1_000 });
  }).toPass();
  await createTask(page, '读历史时开始的任务', 'FAKE:done');
  await expect(page.locator('.attention .card').filter({ hasText: '等你验收' })).toBeVisible();
  await expect(latest).toHaveText('有新内容 · 回到最新 ↓');
  await expect(page.locator('.report.done')).toHaveCount(0);

  await latest.click();
  await expect(latest).toBeHidden();
  const report = page.locator('.report.done');
  await expect(report).toBeVisible();
  await expect(report.locator('details.report-body')).not.toHaveAttribute('open');
  await expect(report).toContainText('写了 work.txt');
});

// A Codex whose administrator does not allow full access refuses to start. One click switches
// Codex to auto review and sends the brief again; the setting stays until the user changes it
// back (harness-adapter.md §1.9).
test('a managed Codex goes on after switching to auto review', async ({ page, backend }) => {
  await open(page, backend);
  await createTask(page, '受管环境的任务', 'FAKE:managed FAKE:done');
  const card = page.locator('.attention .card').filter({ hasText: '没能启动' });
  await expect(card).toContainText('不允许 Codex 以「完全放开」运行');
  await expect(card).toContainText('继续时会原样重新发送');
  await expect(page.locator('.turn-divider').filter({ hasText: '没能启动（权限模式不被允许）' })).toBeVisible();
  await card.getByRole('button', { name: '改用自动审批并继续' }).click();
  await expect(card).toBeHidden();

  const accept = page.locator('.attention .card').filter({ hasText: '受管环境的任务' });
  await expect(accept).toContainText('等你验收');
  await expect(page.locator('.report.done').last()).toContainText('写了 work.txt');

  await page.getByRole('button', { name: '设置', exact: true }).click();
  const settings = page.getByRole('dialog', { name: '设置' });
  const codex = settings.getByRole('group', { name: 'Codex' });
  await expect(codex.getByRole('radio', { name: /^自动审批/ })).toBeChecked();
  // The radio shows the backend's setting, which comes back with the next snapshot.
  await codex.getByRole('radio', { name: /^完全放开/ }).click();
  await expect(codex.getByRole('radio', { name: /^完全放开/ })).toBeChecked();
  await settings.getByRole('button', { name: '关闭' }).click();

  await accept.getByRole('button', { name: '查看并验收' }).click();
  const panel = taskPanel(page, '受管环境的任务');
  await panel.getByRole('button', { name: '验收', exact: true }).click();
  await expect(panel.locator('.phase')).toHaveText('已完成');
});

// A message sent while the executor's turn finishes the task never reaches the executor. It
// does not slip away at acceptance: the user lets it go explicitly (#14).
test('a message the executor never got waits for the user at acceptance', async ({ page, backend }) => {
  await open(page, backend);
  await createTask(page, '补充要求的任务', 'FAKE:wait=2500 FAKE:done FAKE:text=v1');
  await expect(page.getByRole('button', { name: '中断' })).toBeVisible();
  const composer = page.getByLabel('消息');
  await composer.fill('补充：异常行要能看到原始行号');
  await composer.press('Enter');

  const card = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await expect(card).toContainText('还有 1 条你发的消息没交给执行者');
  // The composer says the same: closing the task does not deliver them (#15).
  await expect(composer).toHaveAttribute('placeholder', /退回后交给 Malkuth；直接验收则不再投递/);
  await card.getByRole('button', { name: '查看并验收' }).click();
  const panel = taskPanel(page, '补充要求的任务');
  await expect(panel.locator('.undelivered')).toContainText('异常行要能看到原始行号');
  await expect(panel.getByRole('button', { name: '验收', exact: true })).toHaveCount(0);
  await panel.getByRole('button', { name: '验收，不再投递这些消息' }).click();
  await expect(panel.locator('.phase')).toHaveText('已完成');
  await expect(panel.locator('.history')).toContainText('1 条消息没有投递');
  await panel.getByRole('button', { name: '关闭' }).click();
});

// What a turn outside any task changed waits for the user; its done is shown as having no effect;
// a task made from the changes carries them to the user's repository (#14).
test('changes made outside any task become a task', async ({ page, backend }) => {
  await open(page, backend);
  const composer = page.getByLabel('消息');
  await composer.fill('FAKE:done FAKE:text=outside');
  await composer.press('Enter');
  await expect(page.locator('.report').last()).toContainText('未生效');

  const card = page.locator('.attention .card').filter({ hasText: '在任务之外改了 1 个文件' });
  await expect(card).toContainText('work.txt');
  await card.getByRole('button', { name: '建成任务…' }).click();
  const dialog = page.getByRole('dialog', { name: '用这些改动建任务' });
  await dialog.getByLabel('标题').fill('保留任务之外的改动');
  await dialog.getByLabel('你的原话').fill('FAKE:done FAKE:nowrite');
  await dialog.getByRole('button', { name: '交给 Malkuth' }).click();

  const accept = page.locator('.attention .card').filter({ hasText: '「保留任务之外的改动」通过了验证' });
  await accept.getByRole('button', { name: '查看并验收' }).click();
  const panel = taskPanel(page, '保留任务之外的改动');
  await panel.getByRole('button', { name: '验收', exact: true }).click();
  await expect(panel.locator('.phase')).toHaveText('已完成');
  // The preview writes the file a moment after the acceptance; a fresh repository has none before.
  const file = path.join(backend.repo, 'work.txt');
  await expect.poll(() => fs.existsSync(file) && fs.readFileSync(file, 'utf8'), { timeout: 20_000 }).toBe('outside');
  await panel.getByRole('button', { name: '关闭' }).click();
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

// On a slow machine an older page arrives later than the next render. Scrolling up must still
// load page after page to the start (the baseline found it stuck at the top in CI).
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
