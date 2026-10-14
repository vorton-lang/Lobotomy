// Interface behavior against a mocked backend: the real store, components and WebSocket client,
// fed snapshots, thread pages and task details the test controls. The backend's own rules have
// Rust tests; these check only what the GUI makes of them (AGENTS.md "测试的目的").
import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';
import type { CommandRecord, Item, LogEvent, Message, Push, Snapshot, TaskDetail, Turn } from '../src/api/types';
import { PROJECT, detail, hostSnapshot, role, task } from './mocked';

const origin = process.env.LOBOTOMY_TEST_URL ?? 'http://127.0.0.1:5173';

const turn = (id: string, taskId: string | null): Turn => ({
  id,
  role: 'Malkuth',
  harness: 'codex',
  native_session_id: 'session',
  native_id: null,
  process_start: null,
  workspace_id: 'workspace',
  task_id: taskId,
  attempt_id: null,
  input: '',
  state: 'ended',
  outcome: 'completed',
  failure: null,
  pid: null,
  done_at: null,
  registered_at: 1,
  started_at: 1,
  ended_at: 2,
});

const message = (seq: number, turnId: string, text: string): Item => ({
  id: `i${seq}`,
  seq,
  turn_id: turnId,
  command_id: null,
  created_at: seq,
  kind: 'agent_message',
  content: { text },
});

interface Sent {
  name: string;
  args: Record<string, unknown>;
}

interface State {
  snapshot: Pick<Snapshot, 'roles' | 'tasks' | 'attention'>;
  details?: Record<string, TaskDetail>;
  items?: Item[];
  turns?: Record<string, Turn>;
  commands?: Record<string, CommandRecord>;
  /** Answers a command and changes `state` as the backend would. */
  command?: (sent: Sent) => unknown;
  /** An unsuccessful reply keeps the form's idempotency key for an exact retry. */
  commandError?: string;
}

/**
 * Opens the GUI on a backend that answers from `state`. After a command about a task, it pushes
 * an event for that task, as the backend's event log does once the command has committed.
 */
async function openMocked(page: Page, state: State) {
  let socket!: WebSocketRoute;
  let seq = 1;
  const sent: Sent[] = [];
  const snapshot = (): Snapshot => ({
    seq: 1,
    config: null,
    quota: [],
    harnesses: [],
    live: {},
    project: {
      id: 'project',
      repo_path: '/project',
      branch: 'main',
      author_name: 'Test',
      author_email: 'test@example.com',
      integration: 'base0000',
      integration_rev: 1,
      previewed: 'base0000',
    },
    ...state.snapshot,
  });
  const threadPage = (after?: number, before?: number) => ({
    items: (state.items ?? []).filter(
      (i) => (after === undefined || i.seq > after) && (before === undefined || i.seq < before),
    ),
    has_more: false,
    turns: state.turns ?? {},
    messages: [],
    commands: state.commands ?? {},
    queued: [],
  });
  await page.routeWebSocket(/\/gui\?token=ui-test$/, (route) => {
    socket = route;
    route.onMessage((raw) => {
      const { id, method, params } = JSON.parse(String(raw)) as {
        id: number;
        method: string;
        params: Record<string, any>;
      };
      let result: unknown;
      let changed: string | undefined;
      switch (method) {
        case 'host':
          result = hostSnapshot();
          break;
        case 'snapshot':
          result = snapshot();
          break;
        case 'thread':
          result = threadPage(params.after, params.before);
          break;
        case 'task':
          result = state.details?.[params.task_id];
          break;
        case 'trials':
          result = [];
          break;
        case 'diff':
          result = [];
          break;
        case 'command': {
          const call: Sent = { name: params.name, args: params.args };
          sent.push(call);
          result = state.command?.(call) ?? null;
          changed = typeof call.args.task_id === 'string' ? call.args.task_id : undefined;
          break;
        }
        default:
          route.send(JSON.stringify({ id, error: { code: 'unexpected', message: method } }));
          return;
      }
      if (method === 'command' && state.commandError)
        route.send(JSON.stringify({ id, error: { code: 'unavailable', message: state.commandError } }));
      else route.send(JSON.stringify({ id, result }));
      if (changed) {
        const events: LogEvent[] = [
          {
            seq: ++seq,
            kind: 'task.updated',
            command_id: null,
            caller: null,
            caller_turn_id: null,
            entity: changed,
            payload: { task_id: changed },
          },
        ];
        route.send(JSON.stringify({ type: 'events', project: PROJECT, events } satisfies Push));
      }
    });
  });
  await page.goto(`${origin}/?backend=ws://127.0.0.1:9999/gui&token=ui-test`);
  await expect(page.getByText('已连接', { exact: true })).toBeVisible();
  return {
    sent,
    push(push: Push) {
      socket.send(JSON.stringify(push));
    },
  };
}

// A scroll before the viewport is measured must not look like the user reading back. Later
// pages must keep their last message visible, without even a temporary loss of following.
test('incoming pages keep following while the viewport is first measured', async ({ page }) => {
  const items: Item[] = [];
  const backend = await openMocked(page, {
    snapshot: { roles: [role(null)], tasks: [], attention: [] },
    items,
    turns: { t1: turn('t1', null) },
  });
  await expect(page.locator('.thread.empty')).toContainText('还没有对话');
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 });
  await page.evaluate(() => {
    document.documentElement.dataset.lostFollowing = 'false';
    new MutationObserver(() => {
      if (document.querySelector('.to-latest')) document.documentElement.dataset.lostFollowing = 'true';
    }).observe(document.querySelector('.conversation')!, { subtree: true, childList: true });
  });
  for (let batch = 0; batch < 3; batch++) {
    for (let i = batch * 20 + 1; i <= (batch + 1) * 20; i++) {
      items.push(message(i, 't1', `第 ${i} 条` + (i % 3 === 0 ? '\n\n较长的段落。'.repeat(12) : '')));
    }
    backend.push({ type: 'thread', project: PROJECT, role: 'Malkuth', seq: items.length });
    await expect(page.locator(`[data-key="item-i${items.length}"]`)).toBeInViewport();
  }
  await expect(page.getByRole('button', { name: /回到最新/ })).toBeHidden();
  expect(await page.locator('html').getAttribute('data-lost-following')).toBe('false');
});

// Reading back in the thread, the user stays where they are. A button says when something new
// came and goes back to the end (#18). The report the last message repeats starts folded.
test('back to the latest after reading back', async ({ page }) => {
  const items = Array.from({ length: 60 }, (_, i) =>
    message(i + 1, 't1', `第 ${i + 1} 条` + (i < 50 && i % 3 === 0 ? '\n\n较长的段落。'.repeat(12) : '')),
  );
  const backend = await openMocked(page, {
    snapshot: { roles: [role(null)], tasks: [], attention: [] },
    items,
    turns: { t1: turn('t1', null), t2: turn('t2', 'task1') },
    commands: {
      cmd1: {
        name: 'org_report',
        args: { title: '完成', body: '写了 work.txt', status: 'done', blocked_on: null },
        result: 'recorded',
      },
    },
  });
  const latest = page.getByRole('button', { name: /回到最新/ });
  await expect(page.locator('.thread .message.other').last()).toContainText('第 60 条');
  await expect(latest).toBeHidden();

  // A single wheel gesture must leave the end, including while the jump to the last row is
  // still measuring differently sized messages. Repeated wheels would hide a lost gesture.
  const thread = page.locator('.thread');
  await thread.hover();
  await page.mouse.wheel(0, -100_000);
  await expect(latest).toHaveText('回到最新');

  const area = await thread.boundingBox();
  await latest.click();
  await page.mouse.move(area!.x + area!.width / 2, area!.y + area!.height / 2);
  await page.mouse.wheel(0, -1500);
  // Cross the row-measurement/render boundary before checking that the jump stayed cancelled.
  await page.evaluate(
    () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
  );
  await expect(latest).toHaveText('回到最新');

  // A turn of a new task reports done, then says the same again.
  items.push(
    {
      id: 'i61',
      seq: 61,
      turn_id: 't2',
      command_id: 'cmd1',
      created_at: 61,
      kind: 'mcp_call',
      content: { server: 'lobotomy', tool: 'org_report', arguments: {}, result_text: null, error_text: null },
    },
    message(62, 't2', 'finished'),
  );
  backend.push({ type: 'thread', project: PROJECT, role: 'Malkuth', seq: 62 });
  await expect(latest).toHaveText('有新内容 · 回到最新');
  await expect(page.locator('.report.done')).toHaveCount(0);

  await latest.click();
  await expect(latest).toBeHidden();
  const report = page.locator('.report.done');
  await expect(report).toBeVisible();
  await expect(report.locator('details.report-body')).not.toHaveAttribute('open');
  await expect(report).toContainText('写了 work.txt');
});

// Once the next task starts, the open panel of the accepted one says so and leads to it (#18).
test('a finished task’s panel leads to the task now in work', async ({ page }) => {
  const first = task('task1', '第一个任务', 'accepting');
  const second = task('task2', '第二个任务', 'queued');
  const details: Record<string, TaskDetail> = { task1: detail(first), task2: detail(second) };
  const state: State = {
    snapshot: {
      roles: [role('task1')],
      tasks: [first, second],
      attention: [{ kind: 'accept', task_id: 'task1', title: first.title, verification_id: 'v-task1' }],
    },
    details,
    command: ({ name }) => {
      if (name !== 'accept') throw new Error(name);
      // What the backend does on acceptance: the task closes and the executor takes the next one.
      state.snapshot = {
        roles: [role('task2')],
        tasks: [
          { ...first, phase: 'done' },
          { ...second, phase: 'executing' },
        ],
        attention: [],
      };
      details.task1 = detail({ ...first, phase: 'done' });
      return '2222222222';
    },
  };
  await openMocked(page, state);
  await page
    .locator('.attention .card')
    .filter({ hasText: '「第一个任务」通过了验证' })
    .getByRole('button', { name: '查看并验收' })
    .click();
  const panel = page.getByRole('region', { name: '第一个任务', exact: true });
  await expect(panel.locator('.now-running')).toHaveCount(0);
  await panel.getByRole('button', { name: '验收', exact: true }).click();
  await expect(panel.locator('.phase')).toHaveText('已完成');
  await expect(panel.locator('.now-running')).toContainText('「第二个任务」');
  await panel.getByRole('button', { name: '查看当前任务' }).click();
  await expect(page.getByRole('region', { name: '第二个任务', exact: true })).toBeVisible();
});

// A message the executor never got does not slip away at acceptance: the user lets it go
// explicitly, and acceptance names it (#14).
test('a message the executor never got waits for the user at acceptance', async ({ page }) => {
  const view = task('task1', '补充要求的任务', 'accepting', 1);
  const undelivered: Message = {
    id: 'm1',
    seq: 1,
    role: 'Malkuth',
    source: 'user',
    task_id: 'task1',
    body: '补充：异常行要能看到原始行号',
    turn_id: null,
    created_at: 5,
    reply_to: null,
  };
  const details: Record<string, TaskDetail> = { task1: detail(view, [undelivered]) };
  const state: State = {
    snapshot: {
      roles: [role('task1')],
      tasks: [view],
      attention: [{ kind: 'accept', task_id: 'task1', title: view.title, verification_id: 'v-task1' }],
    },
    details,
    command: () => {
      state.snapshot = {
        roles: [role(null)],
        tasks: [{ ...view, phase: 'done', undelivered_messages: 0 }],
        attention: [],
      };
      details.task1 = detail({ ...view, phase: 'done' });
      return '2222222222';
    },
  };
  const backend = await openMocked(page, state);
  const card = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await expect(card).toContainText('还有 1 条你发的消息没交给执行者');
  // The composer says the same: closing the task does not deliver them (#15).
  await expect(page.getByLabel('消息')).toHaveAttribute('placeholder', /退回后交给 Malkuth；直接验收则不再投递/);

  await card.getByRole('button', { name: '查看并验收' }).click();
  const panel = page.getByRole('region', { name: '补充要求的任务', exact: true });
  await expect(panel.locator('.undelivered')).toContainText('异常行要能看到原始行号');
  await expect(panel.getByRole('button', { name: '验收', exact: true })).toHaveCount(0);
  await panel.getByRole('button', { name: '验收，不再投递这些消息' }).click();
  await expect
    .poll(() => backend.sent.find((s) => s.name === 'accept')?.args)
    .toMatchObject({
      task_id: 'task1',
      verification_id: 'v-task1',
      criteria_version: 1,
      expected_integration: 'base0000',
      dropping: ['m1'],
    });
  await expect(panel.locator('.phase')).toHaveText('已完成');
});

// Delivery rendering uses the existing mock backend; M1 keeps the real create/accept/Git chain.
test('delivery descriptions distinguish pending, older and invalidated rounds', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1400, height: 860 });
  const view = task('delivery', '查看交付内容', 'accepting');
  await openMocked(page, {
    snapshot: {
      roles: [role(view.id)],
      tasks: [view],
      attention: [{ kind: 'accept', task_id: view.id, title: view.title, verification_id: `v-${view.id}` }],
    },
    details: { [view.id]: detail(view) },
  });
  await page
    .locator('.attention .card')
    .filter({ hasText: '等你验收' })
    .getByRole('button', { name: '查看并验收' })
    .click();
  const panel = page.getByRole('region', { name: '查看交付内容', exact: true });
  await expect(panel.getByRole('heading', { name: '本轮交付（第 1 轮）' })).toBeVisible();
  const delivery = panel.getByRole('region', { name: '交付说明' });
  const update = async (state: 'pending' | 'old' | 'new' | 'invalidated' | 'failed') =>
    page.evaluate(async (state) => {
      const module = '/src/store.ts';
      const { useStore } = await import(module);
      const detail = structuredClone(useStore.getState().taskDetail);
      const first = detail.attempts[0];
      first.done_summary =
        '预览入口：[示例页面](https://example.com)\n\n本地文件：`dist/report.html`\n\n体验前运行 `npm run dev`，重点检查页面内容。';
      if (state === 'pending') first.candidate_id = null;
      else first.candidate_id = 'candidate1';
      detail.attempts =
        state === 'old' || state === 'new' || state === 'invalidated'
          ? [
              first,
              {
                ...first,
                id: 'second',
                seq: 2,
                candidate_id: null,
                done_summary: state === 'new' ? '第二轮成果：`output.txt`' : null,
              },
            ]
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

// #67: a lost/unsuccessful reply retries the same fixed action. A new displayed version is
// a new action; retaining the old key would permanently reject it as idempotency_key_reused.
test('display adoption request identity follows the fixed target and internal version', async ({ page }) => {
  const display = {
    state: { kind: 'changed' as const, commit: 'external-a' },
    expected_rev: 1,
    busy: false,
    start_blocked: true,
    unpreviewed: [],
    unavailable: '等待执行者退出、采集保存及任务验收完成' as string | null,
    error: null,
  };
  const state: State = {
    snapshot: { roles: [role(null)], tasks: [], attention: [{ kind: 'display_changed', display }] },
    commandError: '采用回复未收到',
  };
  const backend = await openMocked(page, state);
  const adopt = page.getByRole('button', { name: '采用', exact: true });
  await expect(adopt).toBeDisabled();
  await expect(page.locator('.attention .card')).toContainText('等待执行者退出');
  expect(backend.sent).toHaveLength(0);
  display.unavailable = null;
  backend.push({ type: 'resync' });
  await expect(adopt).toBeEnabled();
  await adopt.click();
  await expect(adopt).toBeEnabled();
  await adopt.click();
  await expect(adopt).toBeEnabled();
  expect(backend.sent[1].args.request_id).toBe(backend.sent[0].args.request_id);
  display.state.commit = 'external-b';
  display.expected_rev = 2;
  backend.push({ type: 'resync' });
  await expect(page.locator('.attention .card')).toContainText('external-b');
  await adopt.click();
  await expect.poll(() => backend.sent.length).toBe(3);
  expect(backend.sent[2].args).toMatchObject({ target: 'external-b', expected_rev: 2 });
  expect(backend.sent[2].args.request_id).not.toBe(backend.sent[0].args.request_id);
});

test('display adoption lists unpreviewed accepted work and confirms the current list', async ({ page }) => {
  const display = {
    state: { kind: 'changed' as const, commit: 'external' },
    expected_rev: 3,
    busy: false,
    start_blocked: true,
    unavailable: null,
    error: null,
    unpreviewed: [{ task_id: 'old-task', title: '尚未展示的修复', commit: 'accepted-old' }],
  };
  const backend = await openMocked(page, {
    snapshot: { roles: [role(null)], tasks: [], attention: [{ kind: 'display_changed', display }] },
  });
  const card = page.locator('.attention .card');
  await expect(card).toContainText('尚未展示的修复');
  await expect(card).toContainText('accepted-old');
  await card.getByRole('button', { name: '采用', exact: true }).click();
  await expect(card.getByRole('button', { name: '确认采用，保留历史' })).toBeVisible();
  expect(backend.sent).toHaveLength(0);
  display.unpreviewed = [{ task_id: 'new-task', title: '新的未展示成果', commit: 'accepted-new' }];
  backend.push({ type: 'resync' });
  await expect(card).toContainText('新的未展示成果');
  await expect(card.getByRole('button', { name: '确认采用，保留历史' })).toHaveCount(0);
  await card.getByRole('button', { name: '采用', exact: true }).click();
  await card.getByRole('button', { name: '取消', exact: true }).click();
  expect(backend.sent).toHaveLength(0);
  await card.getByRole('button', { name: '采用', exact: true }).click();
  await card.getByRole('button', { name: '确认采用，保留历史' }).click();
  await expect.poll(() => backend.sent.length).toBe(1);
  expect(backend.sent[0].args).toMatchObject({ target: 'external', expected_rev: 3, acknowledged: ['accepted-new'] });
});
