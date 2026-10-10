// The trial controls exercise the real renderer and WebSocket client, but never launch a
// terminal in the test runner. Backend process/candidate isolation has its own Rust tests.
import { expect, test, type Page } from '@playwright/test';
import type { Attempt, Capture, Snapshot, TaskDetail, TrialView } from '../src/api/types';
import { hostSnapshot } from './mocked';

const candidate1 = '11111111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const candidate2 = '22222222bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const attempt = (round: number, fixed = true): Attempt => ({
  id: `attempt${round}`, task_id: 'task1', seq: round, started_at: round, ended_at: fixed ? round + 1 : null,
  end_reason: fixed ? 'candidate' : null, code_start: null, done_turn_id: `turn${round}`,
  done_summary: `第 ${round} 轮交付`, candidate_id: fixed ? `capture${round}` : null, conflicts: [],
  trial: { command: `npm run demo:${round}`, purpose: `检查第 ${round} 轮交互` },
});
const capture = (round: number): Capture => ({
  id: `capture${round}`, turn_id: `turn${round}`, role: 'Malkuth', task_id: 'task1', attempt_id: `attempt${round}`,
  kind: 'candidate', base: 'base0000', state: 'pinned', commit_id: round === 1 ? candidate1 : candidate2,
  detail: null, created_at: round + 1, outside: null,
});
const detail = (fixed = true): TaskDetail => ({
  task: {
    id: 'task1', title: '体验候选成果', body: '检查交互', executor: 'Malkuth', phase: 'accepting', paused: false,
    blocked_reason: null, criteria_version: 1, queue_pos: null, revision: 1, created_at: 1, closed_at: null, origin_capture: null,
  },
  criteria: [{ version: 1, text: '交互可用', created_by: 'user', created_at: 1 }],
  attempts: [attempt(1, fixed)], captures: fixed ? [capture(1)] : [], verifications: [], checks: [],
  decisions: [], publications: [], undelivered_messages: [],
});

interface StartArgs { task_id: string; attempt_id: string; expected_candidate: string }
interface Request { id: number; method: string; params: { name?: string; args?: Record<string, unknown> } }

async function openTrial(page: Page, initial = detail()) {
  const api = {
    detail: initial,
    trials: [] as TrialView[],
    requests: [] as Request[],
    list: async (): Promise<TrialView[]> => structuredClone(api.trials),
    start: async (args: StartArgs): Promise<TrialView> => {
      const current = api.detail.attempts.find((item) => item.id === args.attempt_id)!;
      const trial: TrialView = {
        id: `trial${api.trials.length + 1}`, task_id: args.task_id, attempt_id: args.attempt_id,
        round: current.seq, candidate: args.expected_candidate, directory: `/trials/round-${current.seq}`,
        ...current.trial!, state: 'open', error: null,
      };
      api.trials.push(trial);
      return structuredClone(trial);
    },
    stop: async (trialId: string): Promise<null> => {
      api.trials = api.trials.map((trial) => trial.id === trialId ? { ...trial, state: 'closed' } : trial);
      return null;
    },
    update: async (next: TaskDetail) => {
      api.detail = next;
      await page.evaluate(async (value) => {
        const module = '/src/store.ts';
        const { useStore } = await import(module);
        useStore.setState({ taskDetail: value });
      }, next);
    },
  };
  await page.routeWebSocket(/\/gui\?token=trial-test$/, (socket) => {
    socket.onMessage(async (raw) => {
      const request = JSON.parse(String(raw)) as Request;
      api.requests.push(request);
      try {
        let result: unknown;
        switch (request.method) {
          case 'host': result = hostSnapshot(); break;
          case 'snapshot': result = {
            seq: 1,
            project: { id: 'project1', repo_path: '/project', branch: 'main', author_name: 'Test', author_email: 'test@example.com', integration: 'base0000', integration_rev: 1, previewed: 'base0000' },
            config: null, roles: [], tasks: [{ ...api.detail.task, attempt_seq: 1, attempt_open: false, verification: null, undelivered_messages: 0 }],
            attention: [], quota: [], harnesses: [], live: {},
          } satisfies Snapshot; break;
          case 'task': result = api.detail; break;
          case 'diff': result = []; break;
          case 'trials': result = await api.list(); break;
          case 'command':
            if (request.params.name === 'start_trial') result = await api.start(request.params.args as unknown as StartArgs);
            else if (request.params.name === 'stop_trial') result = await api.stop(request.params.args!.trial_id as string);
            else throw new Error(`unexpected command: ${request.params.name}`);
            break;
          default: throw new Error(`unexpected method: ${request.method}`);
        }
        socket.send(JSON.stringify({ id: request.id, result }));
      } catch (error) {
        socket.send(JSON.stringify({ id: request.id, error: { code: 'trial_error', message: String(error) } }));
      }
    });
  });
  await page.goto('http://127.0.0.1:5173/?backend=ws://127.0.0.1:9999/gui&token=trial-test');
  await expect(page.getByText('已连接', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: /体验候选成果/ }).click();
  const panel = page.getByRole('region', { name: '体验候选成果', exact: true });
  const controls = panel.getByRole('region', { name: '一键体验', exact: true });
  await expect(controls).toBeVisible();
  await expect(controls.getByText('正在读取体验状态…')).toBeHidden();
  return { api, panel, controls, commands: (name: string) => api.requests.filter((request) => request.method === 'command' && request.params.name === name) };
}

test('trial launches only a fixed candidate after an explicit click, and stops once', async ({ page }, testInfo) => {
  const { api, controls, commands } = await openTrial(page, detail(false));
  const launch = controls.getByRole('button', { name: '一键体验', exact: true });
  await expect(launch).toBeDisabled();
  await expect(controls).toContainText('候选成果尚未固定');
  await expect(controls).toContainText('npm run demo:1');
  await expect(controls).toContainText('后端所在电脑的桌面终端');
  expect(commands('start_trial')).toHaveLength(0);

  await api.update(detail());
  await expect(controls.getByText('候选版本：')).toContainText('11111111');
  await expect(launch).toBeEnabled();
  expect(commands('start_trial')).toHaveLength(0);
  await page.screenshot({ path: testInfo.outputPath('trial-ready-desktop.png') });

  const start = api.start;
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  api.start = async (args) => { await gate; return start(args); };
  // Two DOM clicks in the same turn also exercise the synchronous pending guard.
  await launch.evaluate((button: HTMLButtonElement) => { button.click(); button.click(); });
  await expect(controls.getByRole('button', { name: '正在打开终端…' })).toBeDisabled();
  await expect.poll(() => commands('start_trial').length).toBe(1);
  expect(commands('start_trial')[0].params.args).toMatchObject({ task_id: 'task1', attempt_id: 'attempt1', expected_candidate: candidate1 });
  release();
  const terminal = controls.getByRole('article', { name: '第 1 轮体验' });
  await expect(terminal).toContainText('终端已打开');
  await expect(controls.getByRole('button', { name: '本轮终端已打开' })).toBeDisabled();
  await expect(terminal).toContainText('/trials/round-1');

  const stop = api.stop;
  let releaseStop!: () => void;
  const stopGate = new Promise<void>((resolve) => { releaseStop = resolve; });
  api.stop = async (id) => { await stopGate; return stop(id); };
  await terminal.getByRole('button', { name: '停止体验' }).evaluate((button: HTMLButtonElement) => { button.click(); button.click(); });
  await expect(terminal.getByRole('button', { name: '正在停止…' })).toBeDisabled();
  await expect.poll(() => commands('stop_trial').length).toBe(1);
  releaseStop();
  await expect(terminal).toContainText('终端已关闭');
  await expect(launch).toBeEnabled();
  expect(commands('start_trial')).toHaveLength(1);
});

test('trial keeps an old terminal when the round changes without reusing its launch recipe', async ({ page }, testInfo) => {
  const { api, panel, controls, commands } = await openTrial(page);
  await controls.getByRole('button', { name: '一键体验', exact: true }).click();
  const old = controls.getByRole('article', { name: '第 1 轮体验' });
  await expect(old).toContainText('终端已打开');

  const next = detail();
  next.attempts.push({ ...attempt(2, false), trial: null, done_summary: null });
  await api.update(next);
  await expect(controls).toContainText('本轮未提供体验命令');
  await expect(controls.getByRole('button', { name: '一键体验', exact: true })).toHaveCount(0);
  await expect(old).toContainText('旧轮体验');
  await expect(old).toContainText('不会自动切换到本轮');
  await expect(old).toContainText('npm run demo:1');
  expect(commands('start_trial')).toHaveLength(1);

  next.attempts[1] = attempt(2, false);
  await api.update(next);
  await expect(controls.getByRole('button', { name: '一键体验', exact: true })).toBeDisabled();
  next.attempts[1] = attempt(2);
  next.captures.push(capture(2));
  await api.update(next);
  await expect(controls.getByText('候选版本：')).toContainText('22222222');
  await expect(controls.getByRole('button', { name: '一键体验', exact: true })).toBeEnabled();
  expect(commands('start_trial')).toHaveLength(1);
  await controls.getByRole('button', { name: '一键体验', exact: true }).click();
  const current = controls.getByRole('article', { name: '第 2 轮体验' });
  await expect(current).toContainText('终端已打开');
  expect(commands('start_trial')[1].params.args).toMatchObject({ attempt_id: 'attempt2', expected_candidate: candidate2 });
  await expect(old).toContainText('/trials/round-1');
  await expect(current).toContainText('/trials/round-2');
  await page.setViewportSize({ width: 1000, height: 780 });
  await page.screenshot({ path: testInfo.outputPath('trial-old-round-compact.png') });

  await panel.getByRole('button', { name: '关闭', exact: true }).click();
  await expect(panel).toBeHidden();
  await page.getByRole('button', { name: /体验候选成果/ }).click();
  await expect(old).toContainText('旧轮体验');
  await expect(current).toContainText('终端已打开');
  await old.getByRole('button', { name: '停止体验' }).click();
  await expect(old).toContainText('终端已关闭');
  await expect(current).toContainText('终端已打开');
  expect(commands('start_trial')).toHaveLength(2);
  expect(commands('stop_trial')[0].params.args).toMatchObject({ trial_id: 'trial1' });
});

test('trial reports launch and stop failures and refreshes closed terminals only while the panel is open', async ({ page }) => {
  await page.clock.install();
  const { api, panel, controls, commands } = await openTrial(page);
  const start = api.start;
  api.start = async () => { throw new Error('桌面终端不可用'); };
  await controls.getByRole('button', { name: '一键体验', exact: true }).click();
  await expect(page.locator('.toast')).toContainText('桌面终端不可用');
  await expect(controls.getByRole('article')).toHaveCount(0);
  await expect(controls.getByRole('button', { name: '一键体验', exact: true })).toBeEnabled();
  api.start = start;
  await controls.getByRole('button', { name: '一键体验', exact: true }).click();
  const terminal = controls.getByRole('article', { name: '第 1 轮体验' });
  await expect(terminal).toContainText('终端已打开');
  api.stop = async () => { throw new Error('未能停止终端'); };
  await terminal.getByRole('button', { name: '停止体验' }).click();
  await expect(page.locator('.toast').filter({ hasText: '未能停止终端' })).toBeVisible();
  await expect(terminal).toContainText('终端已打开');
  await expect(terminal.getByRole('button', { name: '停止体验' })).toBeEnabled();

  const list = api.list;
  api.list = async () => { throw new Error('状态读取失败'); };
  await page.clock.runFor(2_100);
  await expect(controls.getByRole('status')).toContainText('状态读取失败');
  api.trials[0].state = 'closed';
  api.list = list;
  await page.clock.runFor(2_100);
  await expect(terminal).toContainText('终端已关闭');
  await expect(controls.getByRole('button', { name: '一键体验', exact: true })).toBeEnabled();
  await expect(controls.getByRole('status')).toHaveCount(0);
  expect(commands('start_trial')).toHaveLength(2);

  await panel.getByRole('button', { name: '关闭', exact: true }).click();
  const reads = api.requests.filter((request) => request.method === 'trials').length;
  await page.clock.runFor(5_000);
  expect(api.requests.filter((request) => request.method === 'trials')).toHaveLength(reads);
});

test('trial ignores a stale polling response after a launch', async ({ page }) => {
  await page.clock.install();
  const { api, controls, commands } = await openTrial(page);
  let release!: (trials: TrialView[]) => void;
  api.list = () => new Promise((resolve) => { release = resolve; });
  await page.clock.runFor(2_100);
  await expect.poll(() => typeof release).toBe('function');
  await controls.getByRole('button', { name: '一键体验', exact: true }).click();
  await expect(controls.getByRole('article', { name: '第 1 轮体验' })).toContainText('终端已打开');
  release([]);
  // The next reply on the same socket arrives after the stale list was processed.
  await page.evaluate(async () => {
    const module = '/src/store.ts';
    const { call } = await import(module);
    await call('snapshot');
  });
  await expect(controls.getByRole('button', { name: '本轮终端已打开' })).toBeDisabled();
  await expect(controls.getByRole('article', { name: '第 1 轮体验' })).toContainText('终端已打开');
  expect(commands('start_trial')).toHaveLength(1);
});
