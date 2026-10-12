// Not a test: captures the main screens with rich content for design review (notes/ui-ux-plan.md
// §5), against a mocked backend. Run with `SCREENSHOT_DIR=<dir> npx playwright test screenshots`;
// without the variable it is skipped. screenshots.spec.ts covers flows with the real backend.

import { expect, test, type Page } from '@playwright/test';
import type { AngelaStatus, Attention, HostSnapshot, Snapshot, Turn } from '../src/api/types';
import { detail, role, task } from './mocked';

const origin = process.env.LOBOTOMY_TEST_URL ?? 'http://127.0.0.1:5173';
const out = process.env.SCREENSHOT_DIR;
const now = Date.now();
const ago = (minutes: number) => now - minutes * 60_000;

const exported = task('task1', '导出报表', 'accepting');
const login = task('task2', '登录页改版', 'executing');
const queued = { ...task('task3', '清理旧的 CI 缓存', 'queued'), queue_pos: 1 };
const accept: Attention = { kind: 'accept', task_id: 'task1', title: exported.title, verification_id: 'v-task1' };
const blocked: Attention = {
  kind: 'task_blocked',
  task_id: 'task4',
  title: '升级 tokio',
  reason: 'cargo test 有 3 个失败，执行者认为需要你决定是否放弃 0.x 兼容',
};
const quota: Attention = {
  kind: 'quota',
  domain: { harness: 'claude', account: '', blocked_at: ago(20), resets_at: now + 90 * 60_000, message: null },
};
const asking: AngelaStatus = {
  error: null,
  busy: true,
  attention: [
    {
      kind: 'question',
      id: 'q1',
      question: '导出先做 CSV 还是 Excel？两种格式的表头处理不同，会影响 Malkuth 下一步。',
      asked_at: ago(3),
    },
  ],
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
const host: HostSnapshot = {
  projects: [entry('alpha', [accept]), entry('beta', [blocked], true), entry('gamma')],
  attention: [quota],
  harnesses: [],
  angela: asking,
};

const angelaSnapshot: Snapshot = {
  seq: 1,
  quota: [],
  harnesses: [],
  config: null,
  project: null,
  tasks: [],
  attention: [],
  roles: [{ ...role(null), name: 'Angela', kind: 'manager', harness: 'claude', slot: null }],
  live: {
    turn3: {
      role: 'Angela',
      started_at: ago(1),
      items: {
        a: {
          kind: 'tool_call',
          started_at: ago(1),
          content: {
            tool: 'Read',
            input: { file_path: 'C:\\code\\beta\\src\\login.tsx' },
            output: null,
            status: 'in_progress',
          },
        },
      },
    },
  },
};
// Binah's turn has run 14 minutes and shown nothing new for 12.
const projectSnapshot = (id: string): Snapshot => ({
  seq: 1,
  quota: [],
  harnesses: [],
  live: {
    p2: {
      role: 'Binah',
      started_at: ago(14),
      items: { x: { kind: 'agent_message', started_at: ago(12), content: { text: '先看登录表单的校验……' } } },
    },
  },
  config: {
    version: 1,
    config: {
      checks: [{ command: 'npm test', timeout_secs: 600 }],
      excluded: [],
      force_tracked: [],
      max_new_files: 1000,
      max_new_bytes: 50 << 20,
    },
  },
  project: {
    id,
    repo_path: `C:\\code\\${id}`,
    branch: 'main',
    author_name: 'Test',
    author_email: 'test@example.com',
    integration: 'base0000',
    integration_rev: 3,
    previewed: 'base0000',
  },
  roles: [
    role('task1'),
    { ...role('task2'), name: 'Binah', harness: 'claude', unfinished: turn('p2', 'Binah', 14, null, 'task2') },
  ],
  tasks: [exported, { ...login, executor: 'Binah' }, queued],
  attention: [accept],
});

function turn(id: string, who: string, started: number, ended: number | null, taskId: string | null = null): Turn {
  return {
    id,
    role: who,
    harness: 'claude',
    native_session_id: 'session',
    native_id: null,
    process_start: null,
    workspace_id: 'workspace',
    task_id: taskId,
    attempt_id: null,
    input: '',
    state: ended === null ? 'running' : 'ended',
    outcome: ended === null ? null : 'completed',
    failure: null,
    pid: null,
    done_at: null,
    registered_at: ago(started),
    started_at: ago(started),
    ended_at: ended === null ? null : ago(ended),
  };
}
const message = (
  id: string,
  who: string,
  source: string,
  turnId: string | null,
  minutes: number,
  body: string,
  replyTo: string | null = null,
) => ({
  id,
  seq: 0,
  role: who,
  source,
  task_id: null,
  body,
  turn_id: turnId,
  created_at: ago(minutes),
  reply_to: replyTo,
});
const item = (
  id: string,
  seq: number,
  turnId: string,
  minutes: number,
  body: object,
  commandId: string | null = null,
) => ({
  id,
  seq,
  turn_id: turnId,
  command_id: commandId,
  created_at: ago(minutes),
  ...body,
});

const angelaThread = {
  has_more: false,
  turns: {
    turn1: turn('turn1', 'Angela', 42, 39),
    turn2: turn('turn2', 'Angela', 12, 9),
    turn3: turn('turn3', 'Angela', 1, null),
  },
  messages: [
    message(
      'msg_1',
      'Angela',
      'user',
      'turn1',
      42,
      '早。alpha 的导出报表做到哪了？另外 beta 的登录页也该动了，按上周说的那版来。',
    ),
    message('msg_2', 'Angela', 'user', 'turn2', 12, '要带表头，而且列名用中文。', 'item_3'),
  ],
  commands: {},
  queued: [message('msg_3', 'Angela', 'user', null, 0.5, '登录页先别动样式，只改校验逻辑。')],
  items: [
    item('item_1', 1, 'turn1', 41, {
      kind: 'mcp_call',
      content: {
        server: 'lobotomy',
        tool: 'read_project',
        arguments: { project: 'alpha' },
        result_text: '…',
        error_text: null,
        status: 'completed',
      },
    }),
    item('item_2', 2, 'turn1', 40, {
      kind: 'mcp_call',
      content: {
        server: 'lobotomy',
        tool: 'assign',
        arguments: { project: 'beta', title: '登录页改版' },
        result_text: '…',
        error_text: null,
        status: 'completed',
      },
    }),
    item('item_3', 3, 'turn1', 39, {
      kind: 'agent_message',
      content: {
        text:
          '**alpha · 导出报表**：第 1 轮已通过验证，等你验收。\n\n[[att:ver_1]]\n\n**beta · 登录页**：已交给 Binah。你上周说的是：\n\n[[quote:msg_0]]\n\n' +
          '我按这句话理解为"只换布局，不改接口"。如果理解错了直接告诉我。\n\n还有一件事要你定：导出要不要带表头？',
      },
    }),
    item('item_4', 4, 'turn2', 10, {
      kind: 'agent_message',
      content: { text: '收到，已转给 Malkuth，附上了刚才那条的上下文。等它交回来我再告诉你。' },
    }),
  ],
};
const projectThread = {
  has_more: false,
  turns: { p1: turn('p1', 'Malkuth', 30, 22, 'task1') },
  messages: [
    message(
      'pm_1',
      'Malkuth',
      'manager',
      'p1',
      30,
      '【来自 Angela · 任务「导出报表」】\n【用户原话】10-11 22:44\n要带表头，而且列名用中文。\n\n【Angela 解读】\n表头用中文列名；不改公共接口。',
    ),
  ],
  commands: {
    c1: {
      name: 'org_report',
      args: {
        title: '导出报表：加上中文表头',
        body: '表头改为中文列名，`export.rs` 中新增列名映射，测试补了一条。',
        status: 'done',
        blocked_on: null,
      },
      result: 'recorded',
    },
  },
  queued: [],
  items: [
    item('p_1', 1, 'p1', 29, {
      kind: 'tool_call',
      content: {
        tool: 'Read',
        input: { file_path: 'src/export.rs' },
        output: 'pub fn export(…) { … }',
        status: 'completed',
      },
    }),
    item('p_2', 2, 'p1', 27, {
      kind: 'file_change',
      content: {
        changes: [
          { path: 'src/export.rs', kind: 'update' },
          { path: 'tests/export.rs', kind: 'update' },
        ],
        status: 'completed',
      },
    }),
    item('p_3', 3, 'p1', 25, {
      kind: 'command',
      content: {
        command: 'cargo test -p export',
        output: 'running 12 tests\n............\ntest result: ok. 12 passed; 0 failed',
        exit_code: 0,
        status: 'completed',
      },
    }),
    item(
      'p_4',
      4,
      'p1',
      23,
      {
        kind: 'mcp_call',
        content: {
          server: 'lobotomy',
          tool: 'org_report',
          arguments: {},
          result_text: 'recorded',
          error_text: null,
          status: 'completed',
        },
      },
      'c1',
    ),
    item('p_5', 5, 'p1', 22, {
      kind: 'agent_message',
      content: { text: '表头已经改成中文列名，`cargo test` 全部通过。公共接口没有变化。' },
    }),
  ],
};
const refs = {
  quotes: { msg_0: { at: ago(60 * 24 * 5), body: '登录页按 Figma 第二版来，接口别动' } },
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
      at: ago(39),
      superseded: false,
      stale: false,
      unallowed_changes: 0,
    },
  },
};
const forwards = {
  msg_2: [
    {
      project: 'alpha',
      project_name: 'alpha',
      kind: 'message',
      role: 'Malkuth',
      task_id: 'task1',
      task_title: '导出报表',
      at: ago(10),
      contexts: [{ header: 'Angela 13:58（item_3）', chosen: true }],
    },
  ],
};
const said = { source_id: 'msg_1', said_at: ago(42), body: '按上周说的那版来' };
const entryBase = {
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
    { ...entryBase, id: 'led_1', kind: 'goal', statement: '本周把 alpha 的导出和 beta 的登录页交付', evidence: [said] },
    { ...entryBase, id: 'led_2', kind: 'decision', statement: '导出带中文表头', evidence: [said] },
  ],
  projects: [
    {
      id: 'alpha',
      name: 'alpha',
      unavailable: null,
      entries: [
        {
          ...entryBase,
          id: 'led_3',
          kind: 'constraint',
          statement: '不改公共接口',
          task_title: '导出报表',
          evidence: [said],
        },
      ],
    },
    {
      id: 'beta',
      name: 'beta',
      unavailable: null,
      entries: [
        { ...entryBase, id: 'led_4', kind: 'open_question', statement: '登录失败的提示文案由谁定', evidence: [said] },
      ],
    },
  ],
};

async function open(page: Page, width = 1440, height = 900) {
  await page.routeWebSocket(/\/gui\?token=ui-test$/, (route) => {
    route.onMessage((raw) => {
      const { id, method, params } = JSON.parse(String(raw)) as {
        id: number;
        method: string;
        params: Record<string, any>;
      };
      const angela = params.project === 'angela';
      const answers: Record<string, unknown> = {
        host,
        snapshot: angela ? angelaSnapshot : projectSnapshot(params.project),
        thread: angela ? angelaThread : projectThread,
        refs,
        forwards,
        ledger,
        task: detail(exported),
        trials: [],
        diff: [],
        disk_usage: { raw_output: { files: 0, bytes: 0 } },
      };
      route.send(JSON.stringify({ id, result: answers[method] ?? null }));
    });
  });
  await page.setViewportSize({ width, height });
  await page.goto(`${origin}/?backend=ws://127.0.0.1:9999/gui&token=ui-test`);
  await expect(page.getByText('已连接', { exact: true })).toBeVisible();
}

async function shoot(page: Page, name: string) {
  // Markdown shows as plain text until its worker has rendered it.
  await expect(page.locator('.markdown.plain')).toHaveCount(0);
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${out}/${name}-light.png` });
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${out}/${name}-dark.png` });
  await page.emulateMedia({ colorScheme: 'light' });
}

test.describe('screenshots of a mocked backend', () => {
  test.skip(!out, 'review only');

  test('「全部」 with Angela', async ({ page }) => {
    await open(page);
    await expect(page.getByLabel('你的原话').first()).toBeVisible();
    await shoot(page, 'm1-angela');
  });

  test('a project, its thread and a task', async ({ page }) => {
    await open(page);
    await page.getByRole('navigation', { name: '项目' }).getByRole('button', { name: /alpha/ }).click();
    await expect(page.locator('.report')).toBeVisible();
    await shoot(page, 'm2-project');
    await page.locator('.task-open').first().click();
    await expect(page.getByRole('region', { name: '导出报表', exact: true })).toBeVisible();
    await shoot(page, 'm3-task');
  });

  test('a narrow window', async ({ page }) => {
    await open(page, 1024, 760);
    await shoot(page, 'm4-narrow');
  });
});
