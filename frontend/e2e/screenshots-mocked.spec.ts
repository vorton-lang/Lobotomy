// Manual design review of a project, its conversation and a task in light/dark themes.
// Set SCREENSHOT_DIR and run `npx playwright test screenshots-mocked`; otherwise skip before setup.
import { expect, test, type Page } from '@playwright/test';
import type { Snapshot, Turn } from '../src/api/types';
import { PROJECT, detail, hostSnapshot, role, task } from './mocked';

const origin = process.env.LOBOTOMY_TEST_URL ?? 'http://127.0.0.1:5173';
const out = process.env.SCREENSHOT_DIR;
test.skip(!out, 'review only');
const now = Date.now();
const ago = (minutes: number) => now - minutes * 60_000;
const exported = task('task1', '导出报表', 'accepting');
const snapshot: Snapshot = {
  seq: 1,
  quota: [],
  harnesses: [],
  config: null,
  live: {},
  project: {
    id: PROJECT,
    repo_path: '/project',
    branch: 'main',
    author_name: 'Test',
    author_email: 'test@example.com',
    integration: 'base0000',
    integration_rev: 1,
    previewed: 'base0000',
  },
  roles: [role('task1')],
  tasks: [exported],
  attention: [{ kind: 'accept', task_id: 'task1', title: exported.title, verification_id: 'v-task1' }],
};

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
async function open(page: Page) {
  await page.routeWebSocket(/\/gui\?token=ui-test$/, (route) => {
    route.onMessage((raw) => {
      const { id, method } = JSON.parse(String(raw));
      const answers: Record<string, unknown> = {
        host: hostSnapshot(),
        snapshot,
        thread: projectThread,
        refs: { quotes: {}, attachments: {} },
        forwards: {},
        task: detail(exported),
        trials: [],
        diff: [],
      };
      route.send(JSON.stringify({ id, result: answers[method] ?? null }));
    });
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${origin}/?backend=ws://127.0.0.1:9999/gui&token=ui-test`);
  // A cold Vite server can reload after discovering dependencies; wait before the review capture.
  await page.waitForLoadState('networkidle');
  await page.evaluate(() => document.fonts.ready);
  await expect(page.locator('.report')).toBeVisible();
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

test('review a project and its task in wide and narrow windows', async ({ page }) => {
  await open(page);
  await shoot(page, 'project');
  await page.locator('.task-open').first().click();
  await expect(page.getByRole('region', { name: '导出报表', exact: true })).toBeVisible();
  await shoot(page, 'task');
  await page.setViewportSize({ width: 1024, height: 760 });
  await shoot(page, 'task-narrow');
});
