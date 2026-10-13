// #67: an idle display change is visible without first failing an acceptance. The real GUI
// queues work, explicitly adopts the fixed external commit, then completes that queued task.
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { createTask, expect, open, taskPanel, test } from './fixtures';

test('display adoption releases a queued task from the external baseline', async ({ page, backend }) => {
  await open(page, backend);
  fs.writeFileSync(path.join(backend.repo, 'external.txt'), 'external state');
  await expect(page.getByText(/展示目录有未提交修改，请先在外部提交/)).toBeVisible();
  await expect(page.getByRole('button', { name: '接入', exact: true })).toHaveCount(0);
  const git = (...args: string[]) => execFileSync('git', ['-C', backend.repo, ...args], { encoding: 'utf8' }).trim();
  git('add', '.');
  git('commit', '-qm', 'external state');
  const external = git('rev-parse', 'HEAD');
  const card = page.locator('.attention .card').filter({ hasText: '展示分支有外部提交' });
  await expect(card).toContainText(external);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await createTask(page, '外部版本之后', 'FAKE:done');
  await expect(page.locator('.task-line', { hasText: '外部版本之后' })).toContainText('排队');
  await expect(page.locator('.report.done')).toHaveCount(0);
  await card.getByRole('button', { name: '接入', exact: true }).click();
  const acceptance = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await acceptance.getByRole('button', { name: '查看并验收' }).click();
  const panel = taskPanel(page, '外部版本之后');
  await panel.getByRole('button', { name: '验收', exact: true }).click();
  await expect.poll(() => fs.existsSync(path.join(backend.repo, 'work.txt'))).toBe(true);
  expect(fs.readFileSync(path.join(backend.repo, 'external.txt'), 'utf8')).toBe('external state');
  expect(git('rev-parse', 'HEAD^')).toBe(external);
});
