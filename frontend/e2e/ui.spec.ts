// Interface behavior against a mocked backend: the real store, components and WebSocket client,
// fed snapshots, thread pages and task details the test controls. The backend's own rules have
// Rust tests; these check only what the GUI makes of them (AGENTS.md "测试的目的").
import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';
import { devOrigin } from '../dev-server.mjs';
import type { CommandRecord, Item, LogEvent, Message, Push, Snapshot, TaskDetail, Turn } from '../src/api/types';
import type { WorkerTask } from '../src/api/generated';
import { PROJECT, detail, hostSnapshot, role, task } from './mocked';

const origin = process.env.LOBOTOMY_TEST_URL ?? devOrigin;

const turn = (id: string, taskId: string | null): Turn => ({
  id,
  role: 'Malkuth',
  harness: 'codex',
  native_session_id: 'session',
  native_id: null,
  process_start: null,
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
    quota: [],
    harnesses: [],
    live: {},
    project: { id: 'project', repo_path: '/project' },
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
    route.onMessage(async (raw) => {
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
        case 'command': {
          const call: Sent = { name: params.name, args: params.args };
          sent.push(call);
          result = (await state.command?.(call)) ?? null;
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

// The duplicate-submit regression from #16 (9f543fb), independent of the executor's task flow.
test('double-clicking a new task submits the draft once while its reply waits', async ({ page }) => {
  let finish!: () => void;
  const reply = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const worker = { ...role(null), remote: true };
  const backend = await openMocked(page, {
    snapshot: { roles: [worker], tasks: [], attention: [] },
    command: async ({ name }) => {
      expect(name).toBe('create_task');
      await reply;
      return { id: 'created' };
    },
  });
  await page.getByRole('button', { name: '+ 新任务' }).click();
  const form = page.getByRole('dialog', { name: '新任务' });
  await form.getByLabel('标题', { exact: true }).fill('只创建一次');
  await form.getByLabel('你的原话').fill('检查任务的重复提交');
  await form.getByLabel('完成条件').fill('只创建一个任务');
  const submit = form.getByRole('button', { name: '交给 Malkuth' });
  try {
    await submit.dblclick();
    await expect(submit).toBeDisabled();
    expect(backend.sent).toHaveLength(1);
    expect(backend.sent[0]).toMatchObject({
      name: 'create_task',
      args: { title: '只创建一次', body: '检查任务的重复提交', criteria: '只创建一个任务', executor: 'Malkuth' },
    });
  } finally {
    finish();
  }
  await expect(form).toHaveCount(0);
  expect(backend.sent).toHaveLength(1);
});

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
      attention: [{ kind: 'accept', task_id: 'task1', title: view.title }],
    },
    details,
    command: () => {
      state.snapshot = {
        roles: [role(null)],
        tasks: [{ ...view, phase: 'done', undelivered_messages: 0 }],
        attention: [],
      };
      details.task1 = detail({ ...view, phase: 'done' });
      return null;
    },
  };
  const backend = await openMocked(page, state);
  const card = page.locator('.attention .card').filter({ hasText: '等你验收' });
  await expect(card).toContainText('还有 1 条你发的消息没交给执行者');

  await card.getByRole('button', { name: '查看并验收' }).click();
  const panel = page.getByRole('region', { name: '补充要求的任务', exact: true });
  await expect(panel.locator('.undelivered')).toContainText('异常行要能看到原始行号');
  await expect(panel.getByRole('button', { name: '验收', exact: true })).toHaveCount(0);
  await panel.getByRole('button', { name: '验收，不再投递这些消息' }).click();
  await expect
    .poll(() => backend.sent.find((s) => s.name === 'accept')?.args)
    .toMatchObject({
      task_id: 'task1',
      criteria_version: 1,
      dropping: ['m1'],
    });
  await expect(panel.locator('.phase')).toHaveText('已完成');
});

// A worker runs no turns in the project, so its line reads its tasks' attempts; it showed 空闲
// while its task was in CI (found in use, 2026-10-17).
test('a worker on a task is not shown idle', async ({ page }) => {
  const worker: WorkerTask = {
    worker: 'Malkuth',
    spec_version: 1,
    attempt_id: 'a1',
    seq: 1,
    status: { state: 'working', blocked: null, reason: null, question: null },
    artifacts: [],
    evidence: [],
  };
  await openMocked(page, {
    snapshot: {
      roles: [{ ...role(null), remote: true }],
      tasks: [{ ...task('w1', '改一处', 'executing'), worker }],
      attention: [],
    },
  });
  await expect(page.locator('.role-line')).toContainText('在做 1 件');
});

// A worker's task (tech-plan.md §7 step 1): the panel shows the worker's pull request, its checks
// and its account; accepting it merges.
test('a worker task shows its pull request and checks, and accepting merges it', async ({ page }) => {
  const version = 'abcdef1234567890';
  const worker: WorkerTask = {
    worker: 'lobotomy-codex',
    spec_version: 1,
    attempt_id: 'a1',
    seq: 5,
    status: { state: 'delivered', blocked: null, reason: null, question: null },
    artifacts: [
      {
        artifact_id: 'a1:change',
        version,
        kind: 'change',
        supersedes: null,
        title: '加一个 new.txt',
        url: 'https://example.test/o/r/pull/7',
        size: { files: 1, additions: 3, deletions: 0 },
        text: null,
        files: ['new.txt'],
      },
      {
        artifact_id: 'a1:report',
        version: 'r1',
        kind: 'report',
        supersedes: null,
        title: '交付说明',
        url: null,
        size: null,
        text: '加了 new.txt，CI 通过',
        files: [],
      },
    ],
    evidence: [
      {
        artifact_id: 'a1:change',
        version,
        name: 'ci-ok',
        status: 'passed',
        source: 'environment',
        url: 'https://example.test/runs/7',
      },
    ],
  };
  const view = { ...task('w1', '加一个 new.txt', 'accepting'), executor: 'lobotomy-codex', worker };
  const backend = await openMocked(page, {
    snapshot: { roles: [role(null)], tasks: [view], attention: [{ kind: 'accept', task_id: 'w1', title: view.title }] },
    details: { w1: detail(view) },
  });
  await page
    .locator('.attention .card')
    .filter({ hasText: '等你验收' })
    .getByRole('button', { name: '查看并验收' })
    .click();
  const panel = page.getByRole('region', { name: '加一个 new.txt', exact: true });
  const delivery = panel.getByRole('region', { name: 'worker 的交付' });
  await expect(delivery.getByRole('link', { name: '加一个 new.txt' })).toHaveAttribute(
    'href',
    'https://example.test/o/r/pull/7',
  );
  await expect(delivery).toContainText('通过：ci-ok');
  await expect(delivery).toContainText('加了 new.txt，CI 通过');
  await expect(panel.locator('.now')).toContainText('lobotomy-codex：已交付');
  await panel.getByRole('button', { name: '合并', exact: true }).click();
  await expect
    .poll(() => backend.sent.find((s) => s.name === 'accept')?.args)
    .toMatchObject({ task_id: 'w1', criteria_version: 1 });
});
