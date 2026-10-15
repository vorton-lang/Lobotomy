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
const accept: Attention = { kind: 'accept', task_id: 'task1', title: exported.title, verification_id: 'v-task1' };
const quota: Attention = {
  kind: 'quota',
  domain: { harness: 'claude', account: '', blocked_at: 1, resets_at: null, message: null },
};

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
  config: {
    version: 1,
    config: { checks: [], excluded: [], force_tracked: [], max_new_files: 1000, max_new_bytes: 50 << 20 },
  },
  project: {
    id,
    repo_path: `C:\\code\\${id}`,
    branch: 'main',
    author_name: 'Test',
    author_email: 'test@example.com',
    integration: 'base0000',
    integration_rev: 1,
    previewed: 'base0000',
  },
  roles: [role(id === 'alpha' ? 'task1' : null)],
  tasks: id === 'alpha' ? [exported] : [],
  attention: id === 'alpha' ? [accept] : [],
});

const angela: AngelaStatus = { error: null, attention: [], busy: false };

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
    workspace_id: null,
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
          text: '候选成果及验证：\n[[att:candidate]]\n[[att:verification]]\n[[att:accepted]]',
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
        candidate: {
          project: 'alpha',
          project_name: 'alpha',
          kind: 'candidate',
          task_id: 'task1',
          title: '导出报表',
          seq: 1,
          commit: 'abcdef0123456789',
          at: 1,
          superseded: true,
          accepted: false,
          unallowed_changes: 0,
        },
        verification: {
          project: 'alpha',
          project_name: 'alpha',
          kind: 'verification',
          task_id: 'task1',
          title: '导出报表',
          seq: 1,
          state: 'passed',
          base: 'base0000aaaa',
          config_version: 2,
          at: 1,
          superseded: false,
          stale: true,
          accepted: false,
          unallowed_changes: 0,
        },
        // Accepting moved the integration version past it: accepted, not expired.
        accepted: {
          project: 'alpha',
          project_name: 'alpha',
          kind: 'verification',
          task_id: 'task2',
          title: '加行号',
          seq: 1,
          state: 'passed',
          base: 'base1111bbbb',
          config_version: 2,
          at: 1,
          superseded: false,
          stale: true,
          accepted: true,
          unallowed_changes: 0,
        },
      },
    },
  });
  await expect(page.locator('.ref-attachment')).toHaveCount(3);
  await expect(page.locator('.turn-divider.interrupted')).toContainText('被中断');
  await expect(page.locator('.badge.expired')).toHaveCount(2);
  await expect(page.locator('.badge.expired').first()).toContainText('取代');
  await expect(page.locator('.badge.expired').last()).toContainText('已过期');
  await expect(page.locator('.ref-attachment').nth(2).locator('.badge')).toHaveText('已验收');
  await expect(page.locator('.turn-divider.completed')).toHaveText(/^\d{1,2}-\d{1,2} \d{2}:\d{2}$/);
  await expect(page.locator('.ref-attachment').first()).toHaveAttribute('title', /abcdef0123456789/);
  await expect(page.locator('.ref-attachment').nth(1)).toHaveAttribute('title', /base0000aaaa.*检查设置第 2 版/);
  await expect(page.locator('.ref-attachment').nth(1)).not.toContainText('检查设置第 2 版');
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
  config: null,
  project: null,
  tasks: [],
  attention: [],
  roles: [{ ...role(null), name: 'Angela', kind: 'manager', harness: 'claude', slot: null }],
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
      else if (method === 'trials' || method === 'diff') result = [];
      else if (method === 'disk_usage') result = { raw_output: { files: 0, bytes: 0 } };
      else if (method === 'command') result = (await onCommand?.(params.name, params.args)) ?? null;
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
  const host: HostSnapshot = {
    projects: [entry('alpha', [accept]), entry('beta', [], true)],
    attention: [quota],
    harnesses: [],
    angela,
  };
  const backend = await openHost(page, host);

  const list = page.getByRole('navigation', { name: '项目' });
  await expect(list.getByRole('button', { name: /全部/ })).toHaveAttribute('aria-current', 'true');
  await expect(list.getByRole('button', { name: /beta/ }).getByLabel('有 turn 在运行')).toBeVisible();
  const overview = page.getByRole('region', { name: '全部项目' });
  await expect(overview.getByRole('region', { name: '本机' })).toContainText('额度不足（Claude）');
  await expect(overview.getByRole('region', { name: 'alpha' })).toContainText('「导出报表」通过了验证，等你验收');
  await expect(overview.getByRole('region', { name: 'beta' })).toHaveCount(0);
  await expect(page).toHaveTitle('(2) Lobotomy');
  // No project's snapshot: 「全部」 holds Angela's.
  expect(backend.asked.filter((a) => a.method === 'snapshot').map((a) => a.project)).toEqual(['angela']);

  await overview.getByRole('region', { name: 'alpha' }).getByRole('button', { name: '前往' }).click();
  await expect(list.getByRole('button', { name: /alpha/ })).toHaveAttribute('aria-current', 'true');
  await expect(page.getByRole('region', { name: '导出报表', exact: true })).toBeVisible();
  await expect(page.locator('.topbar')).toContainText('alpha · main');

  // Another project: its own snapshot, nothing of the first left over.
  await list.getByRole('button', { name: /beta/ }).click();
  await expect(page.locator('.topbar')).toContainText('beta · main');
  await expect(page.getByRole('region', { name: '导出报表', exact: true })).toHaveCount(0);
  expect(backend.asked.filter((a) => a.method === 'snapshot').map((a) => a.project)).toEqual([
    'angela',
    'alpha',
    'beta',
  ]);
});

// 「全部」 is the conversation with Angela (frontend.md §2): the user's message goes to her, and
// what waits in every project stays beside her.
test('「全部」 is the conversation with Angela beside every project’s items', async ({ page }) => {
  const sent: [string, Record<string, unknown>][] = [];
  const host: HostSnapshot = {
    projects: [entry('alpha', [accept]), entry('beta')],
    attention: [],
    harnesses: [],
    angela,
  };
  const backend = await openHost(page, host, (name, args) => {
    sent.push([name, args]);
    return { id: 'msg1' };
  });
  const conversation = page.getByRole('region', { name: 'Angela', exact: true });
  await conversation.getByLabel('消息').fill('现在怎么样');
  await conversation.getByRole('button', { name: '发送' }).click();
  await expect
    .poll(() => sent.map(([name, args]) => [name, args.role, args.task_id, args.body]))
    .toEqual([['send_message', 'Angela', null, '现在怎么样']]);
  expect(backend.asked.filter((a) => a.method === 'command').map((a) => a.project)).toEqual(['angela']);
  const overview = page.getByRole('region', { name: '全部项目' });
  await expect(overview.getByRole('region', { name: 'alpha' })).toContainText('「导出报表」通过了验证，等你验收');
});

// Angela's reply shows what the runtime renders for her references (manager-actions.md §2.1): the
// user's own words with their time, and one line per attachment with the version it is bound to and
// whether it still holds. An attachment opens its task in its project.
test('Angela’s references show the user’s words and attachments that open the task', async ({ page }) => {
  const host: HostSnapshot = {
    projects: [entry('alpha', [accept]), entry('beta')],
    attention: [],
    harnesses: [],
    angela,
  };
  const text = '你之前说的是：\n[[quote:msg_1]]\n现在的结果：\n[[att:ver_1]]\n[[att:task_gone]]';
  const reply = {
    id: 'i1',
    seq: 1,
    turn_id: 'turn1',
    kind: 'agent_message',
    content: { text },
    command_id: null,
    created_at: 1,
  };
  const refs = {
    quotes: { msg_1: { at: 1, body: '导出要带表头' } },
    attachments: {
      ver_1: {
        project: 'alpha',
        project_name: 'alpha',
        kind: 'verification',
        task_id: 'task1',
        title: '导出报表',
        seq: 1,
        state: 'passed',
        base: 'base0000aaaa',
        config_version: 2,
        at: 1,
        superseded: false,
        stale: true,
        unallowed_changes: 0,
      },
      task_gone: { error: '在打开的项目中找不到；读不到的项目：gamma' },
    },
  };
  const thread = { items: [reply], has_more: false, turns: {}, messages: [], commands: {}, queued: [] };
  const backend = await openHost(page, host, undefined, { thread, refs });

  const conversation = page.getByRole('region', { name: 'Angela', exact: true });
  await expect(conversation.getByLabel('你的原话')).toContainText('导出要带表头');
  const attachment = conversation.getByRole('button', { name: /第 1 轮的验证/ });
  await expect(attachment).toContainText('alpha · 「导出报表」第 1 轮的验证 · 通过');
  await expect(attachment).toHaveAttribute('title', '基于集成版本 base0000aaaa · 检查设置第 2 版');
  await expect(attachment).toContainText('已过期');
  await expect(conversation).toContainText('附件无效：在打开的项目中找不到；读不到的项目：gamma');
  await expect(conversation.locator('.ref-error')).toHaveAttribute('title', 'task_gone');
  expect(backend.asked.filter((a) => a.method === 'refs').map((a) => a.project)).toEqual(['angela']);

  await attachment.click();
  await expect(page.getByRole('navigation', { name: '项目' }).getByRole('button', { name: /alpha/ })).toHaveAttribute(
    'aria-current',
    'true',
  );
  await expect(page.getByRole('region', { name: '导出报表', exact: true })).toBeVisible();
});

// Angela's question waits beside her conversation; the reply goes to her (manager-actions.md §6
// `ask_user`).
test('Angela’s question waits in 「全部」 and the reply goes to her', async ({ page }) => {
  const sent: [string, Record<string, unknown>][] = [];
  const asking: AngelaStatus = {
    ...angela,
    attention: [{ kind: 'question', id: 'q1', question: '导出先做 CSV 还是 Excel？', asked_at: 1 }],
  };
  const host: HostSnapshot = {
    projects: [entry('alpha'), entry('beta')],
    attention: [],
    harnesses: [],
    angela: asking,
  };
  const backend = await openHost(page, host, (name, args) => {
    sent.push([name, args]);
    return { id: 'msg2' };
  });
  const card = page.getByRole('region', { name: '全部项目' }).getByRole('region', { name: 'Angela' });
  await expect(card).toContainText('导出先做 CSV 还是 Excel？');
  await expect(page).toHaveTitle('(1) Lobotomy');
  await card.getByLabel('回复 Angela').fill('先做 CSV');
  await card.getByRole('button', { name: '发送' }).click();
  await expect
    .poll(() => sent.map(([name, args]) => [name, args.role, args.task_id, args.body]))
    .toEqual([['send_message', 'Angela', null, '先做 CSV']]);
  expect(backend.asked.filter((a) => a.method === 'command').map((a) => a.project)).toEqual(['angela']);
});

// The user replies to one of Angela's replies, and sees where she forwarded the message, with the
// context the user chose (manager-actions.md §1, §1.1).
test('a reply to Angela names its target, and the message shows where she forwarded it', async ({ page }) => {
  const sent: [string, Record<string, unknown>][] = [];
  const host: HostSnapshot = {
    projects: [entry('alpha', [accept]), entry('beta')],
    attention: [],
    harnesses: [],
    angela,
  };
  const hers = {
    id: 'item_1',
    seq: 1,
    turn_id: 'turn1',
    kind: 'agent_message',
    content: { text: '导出要不要带表头？\n细节在后面' },
    command_id: null,
    created_at: 1,
  };
  const mine = {
    id: 'msg_2',
    seq: 2,
    role: 'Angela',
    source: 'user',
    task_id: null,
    body: '要',
    turn_id: null,
    created_at: 2,
    reply_to: 'item_1',
  };
  const thread = { items: [hers], has_more: false, turns: {}, messages: [], commands: {}, queued: [mine] };
  const forwards = {
    msg_2: [
      {
        project: 'alpha',
        project_name: 'alpha',
        kind: 'message',
        role: 'Malkuth',
        task_id: 'task1',
        task_title: '导出报表',
        at: 3,
        contexts: [{ header: 'Angela 10-11 13:58（item_1）', chosen: true }],
      },
    ],
  };
  await openHost(
    page,
    host,
    (name, args) => {
      sent.push([name, args]);
      return { id: 'msg_3' };
    },
    { thread, forwards },
  );
  const conversation = page.getByRole('region', { name: 'Angela', exact: true });
  await expect(conversation.locator('.reply-line')).toHaveText('回复：「导出要不要带表头？」');
  const tag = conversation.getByRole('button', { name: /已转给 Malkuth/ });
  await expect(tag).toHaveText('已转给 Malkuth（alpha · 任务「导出报表」） · 附上下文：Angela 10-11 13:58（你指定）');
  await expect(tag).toHaveAttribute('title', 'Angela 10-11 13:58（item_1）');

  await conversation.getByRole('button', { name: '回复', exact: true }).click();
  await expect(conversation.locator('.replying')).toContainText('回复 Angela：「导出要不要带表头？」');
  await conversation.getByLabel('消息').fill('对，带上');
  await conversation.getByRole('button', { name: '发送' }).click();
  await expect.poll(() => sent.map(([, args]) => [args.body, args.reply_to])).toEqual([['对，带上', 'item_1']]);
  await expect(conversation.locator('.replying')).toHaveCount(0);

  await tag.click();
  await expect(page.getByRole('region', { name: '导出报表', exact: true })).toBeVisible();
});

// "共识" beside Angela lists what she recorded and still holds, each with the user's words it rests
// on; a superseded entry is not listed (manager-actions.md §3.3, read-only in M4).
test('共识 lists what Angela recorded and still holds, with the user’s words', async ({ page }) => {
  const host: HostSnapshot = { projects: [entry('alpha'), entry('beta')], attention: [], harnesses: [], angela };
  const said = { source_id: 'msg_1', said_at: 1, body: '导出先做 CSV 吧' };
  const base = {
    task_id: null,
    task_title: null,
    confidence: null,
    supersedes: null,
    superseded_by: null,
    lapsed: false,
    created_at: 1,
  };
  const ledger = {
    user: [
      { ...base, id: 'led_1', kind: 'decision', statement: '导出先做 Excel', superseded_by: 'led_2', evidence: [said] },
      { ...base, id: 'led_2', kind: 'decision', statement: '导出先做 CSV', supersedes: 'led_1', evidence: [said] },
    ],
    projects: [
      {
        id: 'alpha',
        name: 'alpha',
        unavailable: null,
        entries: [
          {
            ...base,
            id: 'led_3',
            kind: 'constraint',
            statement: '不改公共接口',
            task_title: '导出报表',
            evidence: [said],
          },
        ],
      },
      { id: 'beta', name: 'beta', unavailable: null, entries: [] },
    ],
  };
  await openHost(page, host, undefined, { ledger });
  const consensus = page.getByRole('region', { name: '共识' });
  await expect(consensus.getByRole('region', { name: '全局' })).toContainText('决定 导出先做 CSV');
  await expect(consensus).not.toContainText('导出先做 Excel');
  await expect(consensus.getByRole('region', { name: 'alpha' })).toContainText('约束 不改公共接口 · 任务「导出报表」');
  await expect(consensus.getByRole('region', { name: 'beta' })).toHaveCount(0);
  await expect(consensus.locator('.ref-head').first()).toContainText('你的原话');
});

// The projects run without Angela (roles-and-tasks.md §1.4): when she cannot be opened, 「全部」
// says why and lists what waits, as before her.
test('without Angela 「全部」 says why and lists what waits', async ({ page }) => {
  const host: HostSnapshot = {
    projects: [entry('alpha', [accept]), entry('beta')],
    attention: [],
    harnesses: [],
    angela: { error: '数据库是更新版本的 Lobotomy 写的', attention: [], busy: false },
  };
  const backend = await openHost(page, host);
  const overview = page.getByRole('region', { name: '全部项目' });
  await expect(overview).toContainText('Angela 没有打开：数据库是更新版本的 Lobotomy 写的');
  await expect(overview.getByRole('region', { name: 'alpha' })).toContainText('等你验收');
  await expect(page.getByLabel('消息')).toHaveCount(0);
  expect(backend.asked.some((a) => a.method === 'snapshot')).toBe(false);
});

// A new project is connected while the backend runs; the window then shows it.
test('a new project connects from the list and opens', async ({ page }) => {
  const host: HostSnapshot = { projects: [entry('alpha'), entry('beta')], attention: [], harnesses: [], angela };
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
  await expect(page.getByRole('navigation', { name: '项目' }).getByRole('button', { name: /gamma/ })).toHaveAttribute(
    'aria-current',
    'true',
  );
  await expect(page.locator('.topbar')).toContainText('gamma · main');
});

// A project that could not be opened says why in the list and cannot be chosen; it can be opened
// again or archived. An archived one can come back (data-model.md §10.3, §10.4).
test('a project that could not be opened says why; archived ones come back', async ({ page }) => {
  const broken = { ...entry('broken'), error: '数据目录不存在：/data/broken' };
  const old = { ...entry('old'), state: 'archived' as const };
  const sent: [string, unknown][] = [];
  await openHost(
    page,
    { projects: [entry('alpha'), broken, old], attention: [], harnesses: [], angela },
    (name, args) => {
      sent.push([name, args.project_id]);
    },
  );
  const list = page.getByRole('navigation', { name: '项目' });
  await expect(list.getByRole('button', { name: /alpha/ })).toHaveAttribute('aria-current', 'true');
  const failed = list.locator('.project-item.failed');
  await expect(failed).toContainText('数据目录不存在：/data/broken');
  await expect(list.getByRole('button', { name: /^broken/ })).toHaveCount(0);
  await failed.getByRole('button', { name: '重试打开' }).click();
  await failed.getByRole('button', { name: '归档' }).click();
  await list.getByText('已归档 1').click();
  await list.getByRole('button', { name: '取消归档' }).click();
  await expect
    .poll(() => sent)
    .toEqual([
      ['retry_open', 'broken'],
      ['archive_project', 'broken'],
      ['unarchive_project', 'old'],
    ]);
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
  await nav.locator('.settings-nav-group').filter({ hasText: 'beta' }).getByRole('button', { name: '常规' }).click();
  await expect(settings.getByRole('radiogroup', { name: 'Malkuth 使用的 harness' })).toBeVisible();
  expect(backend.asked.filter((a) => a.method === 'snapshot').map((a) => a.project)).toEqual([
    'angela',
    'angela',
    'beta',
  ]);
  // The model and the effort, as the harness lists them (harness-adapter.md §1.2).
  const effort = settings.getByLabel('Malkuth 在 Codex 上的推理强度');
  await expect(effort.locator('option')).toHaveText(['默认强度', 'low', 'medium', 'high']);
  await settings.getByLabel('Malkuth 在 Codex 上的模型').selectOption('gpt-deep');
  await effort.selectOption('high');
  expect(sent.map(([name, { role, harness, model, effort }]) => [name, role, harness, model, effort])).toEqual([
    ['set_role_model', 'Malkuth', 'codex', 'gpt-deep', null],
    ['set_role_model', 'Malkuth', 'codex', null, 'high'],
  ]);

  await settings.getByRole('button', { name: '返回', exact: true }).click();
  await expect(page.locator('.topbar')).toContainText('全部项目');
  await page.getByRole('navigation', { name: '项目' }).getByRole('button', { name: /alpha/ }).click();
  await expect(page.locator('.topbar')).toContainText('alpha · main');
  // From another project, the settings still show beta's page.
  await page.getByRole('button', { name: '设置', exact: true }).click();
  await expect(nav.locator('.settings-nav-group').filter({ hasText: 'beta' }).locator('[aria-current]')).toHaveText(
    '常规',
  );
  await expect(settings.locator('.settings-header h2')).toHaveText('beta');
  expect(errors).toEqual([]);
});

// A form saves explicitly the version it was filled from, to the project of its page.
test('the project configuration is saved to the settings’ project', async ({ page }) => {
  const sent: [string, Record<string, unknown>][] = [];
  const backend = await openHost(
    page,
    { projects: [entry('alpha'), entry('beta')], attention: [], harnesses: [], angela },
    (name, args) => {
      sent.push([name, args]);
      return { version: 2 };
    },
  );
  await page.getByRole('navigation', { name: '项目' }).getByRole('button', { name: /alpha/ }).click();
  await page.getByRole('button', { name: '设置', exact: true }).click();
  const settings = page.getByRole('region', { name: '设置' });
  await settings
    .locator('.settings-nav-group')
    .filter({ hasText: 'beta' })
    .getByRole('button', { name: '验证与采集' })
    .click();
  await settings.getByRole('button', { name: '+ 添加检查命令' }).click();
  await settings.getByLabel('检查命令').fill('npm test');
  await expect(settings.locator('.settings-save')).toContainText('有未保存的修改');
  await settings.getByRole('button', { name: '保存', exact: true }).click();
  await expect(settings.locator('.settings-save')).toContainText('第 2 版');
  expect(
    sent.map(([name, args]) => [
      name,
      args.expected_version,
      (args.config as { checks: { command: string }[] }).checks.map((c) => c.command),
    ]),
  ).toEqual([['edit_project_config', 1, ['npm test']]]);
  expect(backend.asked.filter((a) => a.method === 'command').map((a) => a.project)).toEqual(['beta']);
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
  await expect(page.locator('.topbar')).toContainText('alpha · main');
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
  await expect(page.locator('.topbar')).toContainText('beta · main');
  expect(errors).toEqual([]);
});

// The side columns and the task panel can be dragged wider or narrower; the width stays after a
// reload (frontend.md §2). The arrow keys move the edge too.
test('the right column is dragged wider and keeps its width', async ({ page }) => {
  await openHost(page, { projects: [entry('alpha')], attention: [], harnesses: [], angela });
  await expect(page.locator('.topbar')).toContainText('alpha · main');
  const sidebar = page.locator('.sidebar');
  const sash = page.getByRole('separator', { name: '右侧栏的宽度' });
  expect((await sidebar.boundingBox())!.width).toBeCloseTo(340, 0);
  const box = (await sash.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + 200);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 - 100, box.y + 200, { steps: 5 });
  await page.mouse.up();
  expect((await sidebar.boundingBox())!.width).toBeCloseTo(440, 0);
  await sash.focus();
  await page.keyboard.press('ArrowLeft');
  expect((await sidebar.boundingBox())!.width).toBeCloseTo(456, 0);

  await page.reload();
  await expect(page.locator('.topbar')).toContainText('alpha · main');
  expect((await page.locator('.sidebar').boundingBox())!.width).toBeCloseTo(456, 0);
});
