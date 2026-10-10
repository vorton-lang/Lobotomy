// The project list and 「全部」 against a mocked backend with two projects (frontend.md §2 "M3 的布局").
// The backend's own rules (registration, failure, the one-unarchived-project rule) have Rust tests;
// these check what the window makes of the host snapshot.
import { expect, test, type Page } from '@playwright/test';
import type { Attention, HostSnapshot, Push, Snapshot } from '../src/api/types';
import { detail, role, task } from './mocked';

const origin = process.env.LOBOTOMY_TEST_URL ?? 'http://127.0.0.1:5173';

const exported = task('task1', '导出报表', 'accepting');
const accept: Attention = { kind: 'accept', task_id: 'task1', title: exported.title, verification_id: 'v-task1' };
const quota: Attention = { kind: 'quota', domain: { harness: 'claude', account: '', blocked_at: 1, resets_at: null, message: null } };

const entry = (id: string, attention: Attention[] = [], busy = false): HostSnapshot['projects'][number] => ({
  id, name: id, data_dir: `/data/${id}`, repo_path: `C:\\code\\${id}`, state: 'running', registered_at: 1, error: null, attention, busy,
});

const snapshotOf = (id: string): Snapshot => ({
  seq: 1, quota: [], harnesses: [], live: {},
  config: { version: 1, config: { checks: [], excluded: [], force_tracked: [], max_new_files: 1000, max_new_bytes: 50 << 20 } },
  project: { id, repo_path: `C:\\code\\${id}`, branch: 'main', author_name: 'Test', author_email: 'test@example.com', integration: 'base0000', integration_rev: 1, previewed: 'base0000' },
  roles: [role(id === 'alpha' ? 'task1' : null)],
  tasks: id === 'alpha' ? [exported] : [],
  attention: id === 'alpha' ? [accept] : [],
});

/** Opens the GUI on a backend with the projects `host` lists. */
async function openHost(page: Page, host: HostSnapshot, onCommand?: (name: string, args: Record<string, unknown>) => unknown) {
  const asked: { method: string; project?: string }[] = [];
  let push!: (push: Push) => void;
  await page.routeWebSocket(/\/gui\?token=ui-test$/, (route) => {
    push = (p) => route.send(JSON.stringify(p));
    route.onMessage((raw) => {
      const { id, method, params } = JSON.parse(String(raw)) as { id: number; method: string; params: Record<string, any> };
      asked.push({ method, project: params.project });
      let result: unknown = null;
      if (method === 'host') result = host;
      else if (method === 'snapshot') result = snapshotOf(params.project);
      else if (method === 'thread') result = { items: [], has_more: false, turns: {}, messages: [], commands: {}, queued: [] };
      else if (method === 'task') result = detail(exported);
      else if (method === 'trials' || method === 'diff') result = [];
      else if (method === 'disk_usage') result = { raw_output: { files: 0, bytes: 0 } };
      else if (method === 'command') result = onCommand?.(params.name, params.args) ?? null;
      route.send(JSON.stringify({ id, result }));
    });
  });
  await page.goto(`${origin}/?backend=ws://127.0.0.1:9999/gui&token=ui-test`);
  await expect(page.getByText('已连接', { exact: true })).toBeVisible();
  return { asked, push: (p: Push) => push(p) };
}

// With several projects the window starts on 「全部」: each project's items, the host's blocked quota
// once under 「本机」, and the total in the title. 「前往」 shows the project and opens the task.
test('「全部」 lists every project’s items and goes to one', async ({ page }) => {
  const host: HostSnapshot = { projects: [entry('alpha', [accept]), entry('beta', [], true)], attention: [quota], harnesses: [] };
  const backend = await openHost(page, host);

  const list = page.getByRole('navigation', { name: '项目' });
  await expect(list.getByRole('button', { name: /全部/ })).toHaveAttribute('aria-current', 'true');
  await expect(list.getByRole('button', { name: /beta/ }).getByLabel('有 turn 在运行')).toBeVisible();
  const overview = page.getByRole('region', { name: '全部项目' });
  await expect(overview.getByRole('region', { name: '本机' })).toContainText('额度不足（Claude）');
  await expect(overview.getByRole('region', { name: 'alpha' })).toContainText('「导出报表」通过了验证，等你验收');
  await expect(overview.getByRole('region', { name: 'beta' })).toHaveCount(0);
  await expect(page).toHaveTitle('(2) Lobotomy');
  expect(backend.asked.some((a) => a.method === 'snapshot')).toBe(false);

  await overview.getByRole('region', { name: 'alpha' }).getByRole('button', { name: '前往' }).click();
  await expect(list.getByRole('button', { name: /alpha/ })).toHaveAttribute('aria-current', 'true');
  await expect(page.getByRole('region', { name: '导出报表', exact: true })).toBeVisible();
  await expect(page.locator('.topbar')).toContainText('alpha · main');

  // Another project: its own snapshot, nothing of the first left over.
  await list.getByRole('button', { name: /beta/ }).click();
  await expect(page.locator('.topbar')).toContainText('beta · main');
  await expect(page.getByRole('region', { name: '导出报表', exact: true })).toHaveCount(0);
  expect(backend.asked.filter((a) => a.method === 'snapshot').map((a) => a.project)).toEqual(['alpha', 'beta']);
});

// A new project is connected while the backend runs; the window then shows it.
test('a new project connects from the list and opens', async ({ page }) => {
  const host: HostSnapshot = { projects: [entry('alpha'), entry('beta')], attention: [], harnesses: [] };
  const created: Record<string, unknown>[] = [];
  await openHost(page, host, (name, args) => {
    if (name !== 'create_project') throw new Error(name);
    created.push(args);
    host.projects.push(entry('gamma'));
    return { id: 'gamma' };
  });
  await page.getByRole('button', { name: '+ 新项目' }).click();
  const dialog = page.getByRole('dialog', { name: '新项目' });
  await dialog.getByLabel('仓库路径').fill('C:\\code\\gamma');
  await dialog.getByRole('button', { name: '接入' }).click();
  await expect(dialog).toHaveCount(0);
  expect(created).toEqual([{ repo_path: 'C:\\code\\gamma' }]);
  await expect(page.getByRole('navigation', { name: '项目' }).getByRole('button', { name: /gamma/ })).toHaveAttribute('aria-current', 'true');
  await expect(page.locator('.topbar')).toContainText('gamma · main');
});

// A project that could not be opened says why in the list and cannot be chosen; it can be opened
// again or archived. An archived one can come back (data-model.md §10.3, §10.4).
test('a project that could not be opened says why; archived ones come back', async ({ page }) => {
  const broken = { ...entry('broken'), error: '数据目录不存在：/data/broken' };
  const old = { ...entry('old'), state: 'archived' as const };
  const sent: [string, unknown][] = [];
  await openHost(page, { projects: [entry('alpha'), broken, old], attention: [], harnesses: [] }, (name, args) => {
    sent.push([name, args.project_id]);
  });
  const list = page.getByRole('navigation', { name: '项目' });
  await expect(list.getByRole('button', { name: /alpha/ })).toHaveAttribute('aria-current', 'true');
  const failed = list.locator('.project-item.failed');
  await expect(failed).toContainText('数据目录不存在：/data/broken');
  await expect(list.getByRole('button', { name: /^broken/ })).toHaveCount(0);
  await failed.getByRole('button', { name: '重试打开' }).click();
  await failed.getByRole('button', { name: '归档' }).click();
  await list.getByText('已归档 1').click();
  await list.getByRole('button', { name: '取消归档' }).click();
  await expect.poll(() => sent).toEqual([['retry_open', 'broken'], ['archive_project', 'broken'], ['unarchive_project', 'old']]);
});

// Archiving the project shown stops it; the settings ask once more first.
test('the settings archive the project after asking', async ({ page }) => {
  const sent: [string, unknown][] = [];
  await openHost(page, { projects: [entry('alpha')], attention: [], harnesses: [] }, (name, args) => {
    sent.push([name, args.project_id]);
  });
  await page.getByRole('button', { name: '设置', exact: true }).click();
  const settings = page.getByRole('dialog', { name: '设置' });
  await settings.getByRole('button', { name: '归档这个项目…' }).click();
  expect(sent).toEqual([]);
  await settings.getByRole('button', { name: '归档「alpha」' }).click();
  await expect(settings).toHaveCount(0);
  expect(sent).toEqual([['archive_project', 'alpha']]);
});
