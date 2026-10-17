// The project list and 「全部」 against a mocked backend with two projects (frontend.md §2).
// The backend's own rules (registration, failure, the one-unarchived-project rule) have Rust tests;
// these check what the window makes of the host snapshot. 「全部」 is the conversation with Angela
// (roadmap.md M4 step 1).
import { expect, test, type Page } from '@playwright/test';
import { devOrigin } from '../dev-server.mjs';
import type { AngelaStatus, Attention, HarnessView, HostSnapshot, Push, Snapshot, Turn } from '../src/api/types';
import { detail, role, task } from './mocked';

const origin = process.env.LOBOTOMY_TEST_URL ?? devOrigin;

const exported = task('task1', '导出报表', 'accepting');
const accept: Attention = { kind: 'accept', task_id: 'task1', title: exported.title };

const entry = (id: string, attention: Attention[] = [], busy = false): HostSnapshot['projects'][number] => ({
  id,
  name: id,
  data_dir: `/data/${id}`,
  repo_path: `C:\\code\\${id}`,
  state: 'running',
  registered_at: 1,
  error: null,
  attention,
  busy,
});

const snapshotOf = (id: string): Snapshot => ({
  seq: 1,
  quota: [],
  harnesses: [],
  live: {},
  project: { id, repo_path: `C:\\code\\${id}` },
  roles: [role(id === 'alpha' ? 'task1' : null)],
  tasks: id === 'alpha' ? [exported] : [],
  attention: id === 'alpha' ? [accept] : [],
});

const angela: AngelaStatus = { error: null, attention: [], busy: false };

const claudeModels: HarnessView['models'] = [
  { id: 'claude-fast', label: 'Claude-Fast', efforts: ['low', 'medium', 'high'], default_effort: 'low' },
  { id: 'claude-deep', label: 'Claude-Deep', efforts: ['low', 'medium', 'high', 'xhigh'], default_effort: 'medium' },
];

const codexModels: HarnessView['models'] = [
  { id: 'gpt-fast', label: 'GPT-Fast', efforts: ['low', 'medium', 'high'], default_effort: 'low' },
  { id: 'gpt-deep', label: 'GPT-Deep', efforts: ['low', 'medium', 'high', 'xhigh'], default_effort: 'medium' },
];

test('internal vocabulary stays secondary while abnormal endings remain visible', async ({ page }) => {
  const turn = (id: string, outcome: Turn['outcome']): Turn => ({
    id,
    role: 'Angela',
    harness: 'claude',
    native_session_id: 'session',
    native_id: null,
    task_id: null,
    attempt_id: null,
    input: '',
    state: 'ended',
    outcome,
    failure: null,
    pid: null,
    process_start: null,
    done_at: null,
    registered_at: 1,
    started_at: 1,
    ended_at: 2,
  });
  const item = (id: string, seq: number, turn_id: string, kind: 'agent_message' | 'mcp_call', content: unknown) => ({
    id,
    seq,
    turn_id,
    kind,
    content,
    command_id: null,
    created_at: seq,
  });
  await openHost(page, { projects: [entry('alpha'), entry('beta')], attention: [], harnesses: [], angela }, undefined, {
    thread: {
      items: [
        item('item_3', 1, 'normal', 'agent_message', {
          text: '任务及汇报：\n[[att:task]]\n[[att:late]]',
        }),
        item('tool', 2, 'normal', 'mcp_call', {
          server: 'lobotomy',
          tool: 'assign',
          arguments: { task: '导出报表' },
          result: 'ok',
        }),
        item('external', 3, 'failed', 'mcp_call', { server: 'drive', tool: 'read', arguments: {}, result: 'ok' }),
      ],
      turns: { normal: turn('normal', 'completed'), failed: turn('failed', 'interrupted') },
      messages: [],
      commands: {},
      queued: [],
      has_more: false,
    },
    refs: {
      quotes: {},
      attachments: {
        task: {
          project: 'alpha',
          project_name: 'alpha',
          kind: 'task',
          task_id: 'task1',
          title: '导出报表',
          executor: 'Malkuth',
          phase: 'accepting',
          paused: false,
        },
        late: {
          project: 'alpha',
          project_name: 'alpha',
          kind: 'report',
          task_id: 'task1',
          title: '导出报表',
          role: 'Malkuth',
          report_title: '完成',
          status: 'done',
          at: 1,
          chars: 4,
          effect: 'late',
        },
      },
    },
  });
  await expect(page.locator('.ref-attachment')).toHaveCount(2);
  await expect(page.locator('.turn-divider.interrupted')).toContainText('被中断');
  await expect(page.locator('.badge.expired')).toHaveCount(1);
  await expect(page.locator('.badge.expired')).toHaveText('没有生效');
  await expect(page.locator('.turn-divider.completed')).toHaveText(/^\d{1,2}-\d{1,2} \d{2}:\d{2}$/);
  await expect(page.locator('summary').filter({ hasText: 'assign' })).toHaveText('调用 assign');
  await expect(page.locator('summary').filter({ hasText: 'drive.read' })).toHaveText('调用 drive.read');
  await page.locator('summary').filter({ hasText: 'assign' }).click();
  await expect(page.locator('details[open]')).toContainText('lobotomy.assign');
  await expect(page.locator('.message.other .who')).toHaveAttribute('title', 'item_3');
});

/** Angela's snapshot has the shape of a project's, without a repository and tasks. */
const angelaSnapshot: Snapshot = {
  seq: 1,
  quota: [],
  harnesses: [],
  live: {},
  project: null,
  tasks: [],
  attention: [],
  roles: [{ ...role(null), name: 'Angela', kind: 'manager', harness: 'claude' }],
};

/**
 * Opens the GUI on a backend with the projects `host` lists. `answers` replaces what a read method
 * returns.
 */
async function openHost(
  page: Page,
  host: HostSnapshot,
  onCommand?: (name: string, args: Record<string, unknown>) => unknown,
  answers: Record<string, unknown> = {},
) {
  const asked: { method: string; project?: string }[] = [];
  let push!: (push: Push) => void;
  await page.routeWebSocket(/\/gui\?token=ui-test$/, (route) => {
    push = (p) => route.send(JSON.stringify(p));
    route.onMessage(async (raw) => {
      const { id, method, params } = JSON.parse(String(raw)) as {
        id: number;
        method: string;
        params: Record<string, any>;
      };
      asked.push({ method, project: params.project });
      let result: unknown = null;
      if (method in answers) result = answers[method];
      else if (method === 'host') result = host;
      else if (method === 'snapshot')
        result = params.project === 'angela' ? angelaSnapshot : snapshotOf(params.project);
      else if (method === 'thread')
        result = { items: [], has_more: false, turns: {}, messages: [], commands: {}, queued: [] };
      else if (method === 'task') result = detail(exported);
      else if (method === 'disk_usage') result = { raw_output: { files: 0, bytes: 0 } };
      else if (method === 'command') result = (await onCommand?.(params.name, params.args)) ?? null;
      route.send(JSON.stringify({ id, result }));
    });
  });
  await page.goto(`${origin}/?backend=ws://127.0.0.1:9999/gui&token=ui-test`);
  await expect(page.getByText('已连接', { exact: true })).toBeVisible();
  return { asked, push: (p: Push) => push(p) };
}

// A project's page has no composer, so going there unmounts it; the message being written to
// Angela must still be there on return (found in use, 2026-10-17).
test('a message being written to Angela survives a look at a project', async ({ page }) => {
  await openHost(page, { projects: [entry('alpha'), entry('beta')], attention: [], harnesses: [], angela });
  await page.getByLabel('消息').fill('还没写完的话');
  const projects = page.getByRole('navigation', { name: '项目' });
  await projects.getByRole('button', { name: /alpha/ }).click();
  await expect(page.locator('.topbar')).toContainText('alpha');
  await projects.getByRole('button', { name: /^全部/ }).click();
  await expect(page.getByLabel('消息')).toHaveValue('还没写完的话');
});

// The settings are a page of their own (frontend.md §7.8): they open on the page they showed last,
// wherever the window is, read the project of their page themselves, and leave the window where
// it was.
test('the settings are a page of their own, wherever the window is', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const sent: [string, Record<string, unknown>][] = [];
  const backend = await openHost(
    page,
    {
      projects: [entry('alpha'), entry('beta')],
      attention: [],
      harnesses: [
        { harness: 'codex', permission: 'auto_review', models: codexModels, efforts: ['low', 'medium', 'high'] },
        { harness: 'claude', permission: 'full', models: claudeModels, efforts: ['low', 'medium', 'high'] },
      ],
      angela,
    },
    (name, args) => {
      sent.push([name, args]);
      return null;
    },
  );
  await page.getByRole('button', { name: '设置', exact: true }).click();
  const settings = page.getByRole('region', { name: '设置' });
  await expect(settings.getByRole('heading', { name: '权限' })).toBeVisible();
  await expect(
    settings.getByRole('radiogroup', { name: 'Codex 的权限' }).getByRole('radio', { name: '自动审批' }),
  ).toBeChecked();

  const nav = settings.getByRole('navigation', { name: '设置分类' });
  await nav.getByRole('button', { name: 'Angela' }).click();
  await expect(settings.getByRole('radiogroup', { name: 'Angela 使用的 harness' })).toBeVisible();
  // The model and the effort, as the harness lists them (harness-adapter.md §1.2).
  const effort = settings.getByLabel('Angela 在 Claude 上的推理强度');
  await expect(effort.locator('option')).toHaveText(['默认强度', 'low', 'medium', 'high']);
  await settings.getByLabel('Angela 在 Claude 上的模型').selectOption('claude-deep');
  await effort.selectOption('high');
  expect(sent.map(([name, { role, harness, model, effort }]) => [name, role, harness, model, effort])).toEqual([
    ['set_role_model', 'Angela', 'claude', 'claude-deep', null],
    ['set_role_model', 'Angela', 'claude', null, 'high'],
  ]);
  // A project's page lists no roles: its workers' harness and model are in workers.toml.
  await nav.locator('.settings-nav-group').filter({ hasText: 'beta' }).getByRole('button', { name: '常规' }).click();
  // The repository path comes with beta's snapshot.
  await expect(settings.locator('.settings-header')).toContainText('C:\\code\\beta');
  await expect(settings.getByRole('radiogroup', { name: /使用的 harness/ })).toHaveCount(0);
  expect(backend.asked.filter((a) => a.method === 'snapshot').map((a) => a.project)).toEqual([
    'angela',
    'angela',
    'beta',
  ]);

  await settings.getByRole('button', { name: '返回', exact: true }).click();
  await expect(page.locator('.topbar')).toContainText('全部项目');
  await page.getByRole('navigation', { name: '项目' }).getByRole('button', { name: /alpha/ }).click();
  await expect(page.locator('.topbar')).toContainText('alpha');
  // From another project, the settings still show beta's page.
  await page.getByRole('button', { name: '设置', exact: true }).click();
  await expect(nav.locator('.settings-nav-group').filter({ hasText: 'beta' }).locator('[aria-current]')).toHaveText(
    '常规',
  );
  await expect(settings.locator('.settings-header h2')).toHaveText('beta');
  expect(errors).toEqual([]);
});

// Archiving from the settings asks once more. A host push can announce the archive before the
// command's reply: the page of the archived project goes, the settings stay usable on the host's
// page, and the window goes to 「全部」. The reply is held until then, so the order is tested
// without a timer.
test('archiving from the settings asks first and leaves them usable', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const host: HostSnapshot = {
    projects: [entry('alpha', [], true), entry('beta')],
    attention: [],
    harnesses: [
      { harness: 'codex', permission: 'auto_review', models: codexModels, efforts: ['low', 'medium', 'high'] },
    ],
    angela,
  };
  const sent: unknown[] = [];
  let finish!: () => void;
  const reply = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const backend = await openHost(page, host, async (name, args) => {
    if (name !== 'archive_project') return null;
    sent.push(args.project_id);
    host.projects[0].state = 'archived';
    backend.push({ type: 'host' });
    await reply;
    return null;
  });
  await page.getByRole('navigation', { name: '项目' }).getByRole('button', { name: /alpha/ }).click();
  await expect(page.locator('.topbar')).toContainText('alpha');
  await page.getByRole('button', { name: '设置', exact: true }).click();
  const settings = page.getByRole('region', { name: '设置' });
  await settings
    .locator('.settings-nav-group')
    .filter({ hasText: 'alpha' })
    .getByRole('button', { name: '常规' })
    .click();
  await settings.getByRole('button', { name: '归档…' }).click();
  expect(sent).toEqual([]);
  await settings.getByRole('button', { name: '归档「alpha」' }).click();
  try {
    await expect(settings.getByRole('heading', { name: '权限' })).toBeVisible();
    await expect(settings.locator('.settings-nav-group').filter({ hasText: 'alpha' })).toHaveCount(0);
    await expect(page.locator('.topbar')).toContainText('全部项目');
  } finally {
    finish();
  }
  expect(sent).toEqual(['alpha']);
  await settings.getByRole('button', { name: '返回', exact: true }).click();
  await expect(page.getByRole('navigation', { name: '项目' })).toContainText('已归档 1');
  await page.getByRole('navigation', { name: '项目' }).getByRole('button', { name: /beta/ }).click();
  await expect(page.locator('.topbar')).toContainText('beta');
  expect(errors).toEqual([]);
});
