import { afterEach, expect, test, vi } from 'vitest';
import type { ConnectionEvents } from './api/connection';
import type {
  AngelaStatus,
  Attention,
  HostSnapshot,
  Item,
  Message,
  RoleView,
  Snapshot,
  ThreadPage,
  Turn,
} from './api/types';

interface Pending {
  method: string;
  params: Record<string, unknown>;
  resolve: (value: unknown) => void;
}

const probe = vi.hoisted(() => ({
  events: null as ConnectionEvents | null,
  pending: [] as Pending[],
}));

vi.mock('./api/connection', async (original) => ({
  ...(await original<typeof import('./api/connection')>()),
  Connection: class {
    constructor(_url: string, events: ConnectionEvents) {
      probe.events = events;
    }
    command(params: Record<string, unknown>) {
      return this.call('command', params);
    }
    start() {}
    stop() {}
    call(method: string, params: Record<string, unknown>) {
      return new Promise((resolve) => probe.pending.push({ method, params, resolve }));
    }
  },
}));

import {
  ANGELA,
  chooseProject,
  connect,
  loadOlder,
  scope,
  selectProject,
  run,
  totalAttention,
  useStore,
} from './store';

afterEach(() => {
  // A debounced refresh still waiting would swallow the next test's.
  if (vi.isFakeTimers()) vi.runOnlyPendingTimers();
  vi.useRealTimers();
  probe.pending = [];
  useStore.setState({ host: null, project: null, snapshot: null, live: {} });
});

const snapshot = (permission: 'full' | 'auto_review'): Snapshot => ({
  seq: 1,
  project: null,
  roles: [],
  tasks: [],
  attention: [],
  quota: [],
  harnesses: [{ harness: 'codex', permission, models: [], efforts: [] }],
  live: {},
});

const project = (
  id: string,
  extra: Partial<HostSnapshot['projects'][number]> = {},
): HostSnapshot['projects'][number] => ({
  id,
  name: id,
  data_dir: `/data/${id}`,
  repo_path: `/repo/${id}`,
  state: 'running',
  registered_at: 1,
  error: null,
  attention: [],
  busy: false,
  ...extra,
});

/** Angela as the host shows her: open, or not with the reason. */
const angela = (error: string | null = null, attention: Attention[] = []): AngelaStatus => ({
  error,
  attention,
  busy: false,
});

const host = (...ids: string[]): HostSnapshot => ({
  projects: ids.map((id) => project(id)),
  attention: [],
  harnesses: [],
  angela: angela(),
});

/** Answers the oldest request of this method. */
async function answer(method: string, value: unknown) {
  const at = probe.pending.findIndex((p) => p.method === method);
  expect(at, `a pending ${method} request`).toBeGreaterThanOrEqual(0);
  probe.pending.splice(at, 1)[0].resolve(value);
  await vi.advanceTimersByTimeAsync(0);
}

const pending = (method: string) => probe.pending.filter((p) => p.method === method);

test('a superseded snapshot response does not replace the newer snapshot or live view', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  useStore.setState({ project: 'p1' });
  const events = probe.events!;
  // Host changes have no project event: both snapshots can have the same event sequence.
  events.push({ type: 'host' });
  await vi.advanceTimersByTimeAsync(120);
  await answer('host', host('p1'));
  events.push({ type: 'host' });
  await vi.advanceTimersByTimeAsync(120);
  await answer('host', host('p1'));
  expect(pending('snapshot')).toHaveLength(2);

  const [older, latest] = pending('snapshot');
  const newer = snapshot('auto_review');
  latest.resolve(newer);
  await vi.advanceTimersByTimeAsync(0);
  expect(useStore.getState().snapshot).toBe(newer);
  expect(useStore.getState().live).toBe(newer.live);

  older.resolve(snapshot('full'));
  await vi.advanceTimersByTimeAsync(0);
  expect(useStore.getState().snapshot).toBe(newer);
  expect(useStore.getState().live).toBe(newer.live);
});

test('requests about the project name it; the host snapshot does not', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  probe.events!.opened();
  const requests = pending('host');
  expect(requests).toHaveLength(1);
  expect(requests[0].params).not.toHaveProperty('project');
  await answer('host', host('p1'));
  expect(useStore.getState().project).toBe('p1');
  expect(pending('snapshot')[0].params).toEqual({ project: 'p1' });
  await answer('snapshot', snapshot('full'));
});

test('a reply about the project the window left is dropped', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  useStore.setState({ host: host('p1', 'p2'), project: 'p1' });
  probe.events!.push({
    type: 'events',
    project: 'p1',
    events: [
      { seq: 2, kind: 'task.updated', command_id: null, caller: null, caller_turn_id: null, entity: 't', payload: {} },
    ],
  });
  await vi.advanceTimersByTimeAsync(120);
  expect(pending('snapshot')[0].params).toEqual({ project: 'p1' });

  const switching = selectProject('p2');
  await vi.advanceTimersByTimeAsync(0);
  expect(pending('snapshot')[1].params).toEqual({ project: 'p2' });
  // The reply about p1 comes after the switch.
  await answer('snapshot', snapshot('full'));
  expect(useStore.getState().snapshot).toBe(null);

  const current = snapshot('auto_review');
  await answer('snapshot', current);
  await switching;
  expect(useStore.getState().project).toBe('p2');
  expect(useStore.getState().snapshot).toBe(current);
});

test('another project’s events refresh the counts, not the window', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  useStore.setState({ host: host('p1', 'p2'), project: 'p1' });
  probe.events!.push({
    type: 'events',
    project: 'p2',
    events: [
      { seq: 2, kind: 'task.updated', command_id: null, caller: null, caller_turn_id: null, entity: 't', payload: {} },
    ],
  });
  await vi.advanceTimersByTimeAsync(300);
  expect(pending('host')).toHaveLength(1);
  expect(pending('snapshot')).toHaveLength(0);
});

test('pushes about another project do not change the window', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  useStore.setState({ project: 'p1', live: {} });
  probe.events!.push({ type: 'live', project: 'p2', live: { t: { role: 'Malkuth', started_at: 1, items: {} } } });
  expect(useStore.getState().live).toEqual({});
  probe.events!.push({ type: 'live', project: 'p1', live: { t: { role: 'Malkuth', started_at: 1, items: {} } } });
  expect(Object.keys(useStore.getState().live)).toEqual(['t']);
});

test('the window keeps its project while it stays open; at start it shows the only open one', () => {
  const failed = project('p1', { error: '数据目录不存在' });
  const archived = project('p2', { state: 'archived' });
  const others = { projects: [failed, archived, project('p3')], attention: [], harnesses: [], angela: angela() };
  expect(chooseProject(host('p1', 'p2'), 'p2', false)).toBe('p2');
  // A project that closes leaves the window on 「全部」.
  expect(chooseProject(others, 'p1', false)).toBe(null);
  expect(chooseProject(others, null, false)).toBe(null);
  // At start: the only open project, else 「全部」.
  expect(chooseProject(others, null, true)).toBe('p3');
  expect(chooseProject(host('p1', 'p2'), null, true)).toBe(null);
  expect(
    chooseProject({ projects: [failed, archived], attention: [], harnesses: [], angela: angela() }, null, true),
  ).toBe(null);
});

test('the total counts each project’s items, Angela’s, and the host’s quota domains once', () => {
  const accept = { kind: 'accept', task_id: 't', title: 'T' } as const;
  const quota = {
    kind: 'quota',
    domain: { harness: 'claude', account: '', blocked_at: 1, resets_at: null, message: null },
  } as const;
  const held: Attention = { kind: 'unknown_turn', role: 'Angela', turn_id: 'turn1' };
  const counted: HostSnapshot = {
    projects: [project('p1', { attention: [accept] }), project('p2', { attention: [accept, accept] })],
    attention: [quota],
    harnesses: [],
    angela: angela(null, [held]),
  };
  expect(totalAttention(counted)).toBe(5);
  expect(totalAttention(null)).toBe(0);
});

// 「全部」 is the conversation with Angela (frontend.md §2): its requests name her. Without her it is
// the list of what waits, which needs no snapshot.
test('on 「全部」 requests name Angela while she is open', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  probe.events!.opened();
  await answer('host', host('p1', 'p2'));
  expect(useStore.getState().project).toBe(null);
  expect(scope()).toBe(ANGELA);
  expect(pending('snapshot')[0].params).toEqual({ project: ANGELA });
  await answer('snapshot', snapshot('full'));

  // Her pushes are the window's; a project's are not.
  probe.events!.push({ type: 'live', project: 'p1', live: { t: { role: 'Malkuth', started_at: 1, items: {} } } });
  expect(useStore.getState().live).toEqual({});
  probe.events!.push({ type: 'live', project: ANGELA, live: { t: { role: 'Angela', started_at: 1, items: {} } } });
  expect(Object.keys(useStore.getState().live)).toEqual(['t']);
});

test('on 「全部」 without Angela nothing names her', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  probe.events!.opened();
  await answer('host', { ...host('p1', 'p2'), angela: angela('数据库是更新版本的 Lobotomy 写的') });
  expect(scope()).toBe(null);
  expect(pending('snapshot')).toHaveLength(0);
});

const turnEvent = (project: string) => ({
  type: 'events' as const,
  project,
  events: [
    { seq: 2, kind: 'turn.ended', command_id: null, caller: null, caller_turn_id: null, entity: 't1', payload: {} },
  ],
});

const turn = (state: 'running' | 'ended') => ({ id: 't1', role: 'Malkuth', state }) as Turn;
const item = (id: string, seq: number) => ({ id, seq, turn_id: 't1', kind: 'agent_message' }) as Item;
const queued = { id: 'm1', role: 'Malkuth', turn_id: null } as Message;
const threadPage = (items: Item[], state: 'running' | 'ended', has_more = false): ThreadPage => ({
  items,
  has_more,
  turns: { t1: turn(state) },
  messages: [],
  commands: {},
  queued: state === 'running' ? [queued] : [],
});

/** The window shows p1's thread: turn t1 runs and a message waits for it. */
function showThread() {
  useStore.setState({
    host: host('p1', 'p2'),
    project: 'p1',
    snapshot: { ...snapshot('full'), roles: [{ name: 'Malkuth', kind: 'worker' } as RoleView] },
    thread: {
      role: 'Malkuth',
      items: [item('b', 5)],
      turns: { t1: turn('running') },
      messages: [],
      commands: {},
      queued: [queued],
      hasMore: true,
      loaded: true,
    },
  });
}

test('a thread page read earlier does not bring back an older turn state or queue', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  showThread();
  const older = loadOlder();
  // Two refreshes of the turn states; the second does not wait for the first.
  probe.events!.push(turnEvent('p1'));
  await vi.advanceTimersByTimeAsync(120);
  probe.events!.push(turnEvent('p1'));
  await vi.advanceTimersByTimeAsync(120);
  const [page, first, second] = pending('thread');
  expect(page.params).toMatchObject({ before: 5 });

  second.resolve(threadPage([item('b', 5)], 'ended'));
  await vi.advanceTimersByTimeAsync(0);
  first.resolve(threadPage([item('b', 5)], 'running'));
  page.resolve(threadPage([item('a', 4)], 'running'));
  await older;
  const thread = useStore.getState().thread;
  expect(thread.turns.t1.state).toBe('ended');
  expect(thread.queued).toEqual([]);
  // The older page still goes in front.
  expect(thread.items.map((i) => i.id)).toEqual(['a', 'b']);
});

test('an older page answered first does not drop the newest page read before it', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  showThread();
  probe.events!.push(turnEvent('p1'));
  await vi.advanceTimersByTimeAsync(120);
  const older = loadOlder();
  const [newest, page] = pending('thread');
  expect(page.params).toMatchObject({ before: 5 });

  // The older page carries only the turns of its items.
  page.resolve({ ...threadPage([item('a', 4)], 'running'), turns: {}, queued: [] });
  await older;
  newest.resolve(threadPage([item('b', 5)], 'ended'));
  await vi.advanceTimersByTimeAsync(0);
  const thread = useStore.getState().thread;
  expect(thread.turns.t1.state).toBe('ended');
  expect(thread.queued).toEqual([]);
  expect(thread.items.map((i) => i.id)).toEqual(['a', 'b']);
});

test('a thread read from an earlier visit to the project does not apply after the window returns', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  showThread();
  probe.events!.push(turnEvent('p1'));
  await vi.advanceTimersByTimeAsync(120);
  const [before] = pending('thread');
  probe.pending = [];

  const away = selectProject('p2');
  await answer('snapshot', snapshot('full'));
  await away;
  const back = selectProject('p1');
  await answer('snapshot', { ...snapshot('full'), roles: [{ name: 'Malkuth', kind: 'worker' } as RoleView] });
  await answer('thread', threadPage([item('b', 5)], 'ended'));
  await back;
  // The refresh from the first visit answers last.
  before.resolve(threadPage([item('b', 5)], 'running'));
  await vi.advanceTimersByTimeAsync(0);
  const thread = useStore.getState().thread;
  expect(thread.turns.t1.state).toBe('ended');
  expect(thread.queued).toEqual([]);
});

test('typed commands keep the project and supplied retry id', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  useStore.setState({ project: 'p1' });
  const created = run('create_task', { title: 'Task', body: '', criteria: '', executor: 'Malkuth' }, 'retry-id');
  expect(pending('command')[0].params).toEqual({
    name: 'create_task',
    project: 'p1',
    args: { request_id: 'retry-id', title: 'Task', body: '', criteria: '', executor: 'Malkuth' },
  });
  await answer('command', { id: 'task' });
  expect(await created).toEqual({ id: 'task' });
});
