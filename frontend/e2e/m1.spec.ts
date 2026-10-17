// Angela's conversation through the GUI against a real backend: long threads that only the fake
// Codex's turns make. Each test has a backend of its own (fixtures.ts).

import { expect, open, showAngela, test } from './fixtures';

// On a slow machine an older page arrives later than the next render. Scrolling up must still
// load page after page to the start (the baseline found it stuck at the top in CI).
// Run the longest independent scenario early so it does not leave one worker idle at the end.
test('scrolling up on a slow machine loads every older page', async ({ page, backend }) => {
  await open(page, backend);
  await showAngela(page);
  // Three hundred items: the thread opens three pages after the first message.
  const composer = page.getByLabel('消息');
  await composer.fill('很长的对话从这里开始 FAKE:many=300 FAKE:marker=newest');
  await composer.press('Enter');
  await expect(page.locator('.thread')).toContainText('bench newest');
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 });
  await page.reload();
  await expect(page.getByText('已连接')).toBeVisible();
  await showAngela(page);
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
    '很长的对话从这里开始',
  );
  expect(reached).toBe(true);
});

// A command that writes a file through a heredoc takes a line or two until opened; search still
// reaches its end (#14). Its whole text opens over the window and copies whole (#15).
test('a long command stays short until opened', async ({ page, backend }) => {
  await open(page, backend);
  await showAngela(page);
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
