// Interface behavior against a mocked backend: the real store, components and WebSocket client,
// fed snapshots, thread pages and task details the test controls. The backend's own rules have
// Rust tests; these check only what the GUI makes of them (AGENTS.md "测试的目的").
import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';
import type { CommandRecord, Item, LogEvent, Message, Push, Snapshot, TaskDetail, Turn } from '../src/api/types';
import { PROJECT, detail, hostSnapshot, role, task } from './mocked';

const origin = process.env.LOBOTOMY_TEST_URL ?? 'http://127.0.0.1:5173';

const turn = (id: string, taskId: string | null): Turn => ({
  id, role: 'Malkuth', harness: 'codex', task_id: taskId, attempt_id: null, input: '', state: 'ended', outcome: 'completed',
  failure: null, pid: null, done_at: null, registered_at: 1, started_at: 1, ended_at: 2,
});

const message = (seq: number, turnId: string, text: string): Item => ({
  id: `i${seq}`, seq, turn_id: turnId, command_id: null, created_at: seq, kind: 'agent_message', content: { text },
});

interface Sent { name: string; args: Record<string, unknown> }

interface State {
  snapshot: Pick<Snapshot, 'roles' | 'tasks' | 'attention'>;
  details?: Record<string, TaskDetail>;
  items?: Item[];
  turns?: Record<string, Turn>;
  commands?: Record<string, CommandRecord>;
  /** Answers a command and changes `state` as the backend would. */
  command?: (sent: Sent) => unknown;
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
    seq: 1, config: null, quota: [], harnesses: [], live: {},
    project: { id: 'project', repo_path: '/project', branch: 'main', author_name: 'Test', author_email: 'test@example.com', integration: 'base0000', integration_rev: 1, previewed: 'base0000' },
    ...state.snapshot,
  });
  const threadPage = (after?: number, before?: number) => ({
    items: (state.items ?? []).filter((i) => (after === undefined || i.seq > after) && (before === undefined || i.seq < before)),
    has_more: false, turns: state.turns ?? {}, messages: [], commands: state.commands ?? {}, queued: [],
  });
  await page.routeWebSocket(/\/gui\?token=ui-test$/, (route) => {
    socket = route;
    route.onMessage((raw) => {
      const { id, method, params } = JSON.parse(String(raw)) as { id: number; method: string; params: Record<string, any> };
      let result: unknown;
      let changed: string | undefined;
      switch (method) {
        case 'host': result = hostSnapshot(); break;
        case 'snapshot': result = snapshot(); break;
        case 'thread': result = threadPage(params.after, params.before); break;
        case 'task': result = state.details?.[params.task_id]; break;
        case 'trials': result = []; break;
        case 'diff': result = []; break;
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
      route.send(JSON.stringify({ id, result }));
      if (changed) {
        const events: LogEvent[] = [{ seq: ++seq, kind: 'task.updated', entity: changed, payload: { task_id: changed } }];
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

// Reading back in the thread, the user stays where they are. A button says when something new
// came and goes back to the end (#18). The report the last message repeats starts folded.
test('back to the latest after reading back', async ({ page }) => {
  const items = Array.from({ length: 60 }, (_, i) => message(i + 1, 't1', `第 ${i + 1} 条`));
  const backend = await openMocked(page, {
    snapshot: { roles: [role(null)], tasks: [], attention: [] },
    items,
    turns: { t1: turn('t1', null), t2: turn('t2', 'task1') },
    commands: { cmd1: { name: 'org_report', args: { title: '完成', body: '写了 work.txt', status: 'done', blocked_on: null }, result: 'recorded' } },
  });
  const latest = page.getByRole('button', { name: /回到最新/ });
  await expect(page.locator('.thread .message.other').last()).toContainText('第 60 条');
  await expect(latest).toBeHidden();

  // Scroll up with the wheel, as the user does. Just after rows render, the list still goes to the
  // end each time a row's height changes (virtua's scrollToIndex, for about 150 ms). A scroll made
  // then is undone, so scroll again until the list stays where it was scrolled to.
  await page.locator('.thread').hover();
  await expect(async () => {
    await page.mouse.wheel(0, -100_000);
    await expect(latest).toHaveText('回到最新 ↓', { timeout: 1_000 });
  }).toPass();

  // A turn of a new task reports done, then says the same again.
  items.push(
    { id: 'i61', seq: 61, turn_id: 't2', command_id: 'cmd1', created_at: 61, kind: 'mcp_call', content: { server: 'lobotomy', tool: 'org_report', arguments: {}, result_text: null, error_text: null } },
    message(62, 't2', 'finished'),
  );
  backend.push({ type: 'thread', project: PROJECT, role: 'Malkuth', seq: 62 });
  await expect(latest).toHaveText('有新内容 · 回到最新 ↓');
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
        tasks: [{ ...first, phase: 'done' }, { ...second, phase: 'executing' }],
        attention: [],
      };
      details.task1 = detail({ ...first, phase: 'done' });
      return '2222222222';
    },
  };
  await openMocked(page, state);
  await page.locator('.attention .card').filter({ hasText: '「第一个任务」通过了验证' }).getByRole('button', { name: '查看并验收' }).click();
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
    id: 'm1', seq: 1, role: 'Malkuth', source: 'user', task_id: 'task1', body: '补充：异常行要能看到原始行号', turn_id: null, created_at: 5,
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
      state.snapshot = { roles: [role(null)], tasks: [{ ...view, phase: 'done', undelivered_messages: 0 }], attention: [] };
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
  await expect.poll(() => backend.sent.find((s) => s.name === 'accept')?.args).toMatchObject({
    task_id: 'task1', verification_id: 'v-task1', criteria_version: 1, expected_integration: 'base0000', dropping: ['m1'],
  });
  await expect(panel.locator('.phase')).toHaveText('已完成');
});
