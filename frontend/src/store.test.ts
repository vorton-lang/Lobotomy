import { afterEach, expect, test, vi } from 'vitest';
import type { ConnectionEvents } from './api/connection';
import type { HostSnapshot, Snapshot } from './api/types';

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
    constructor(_url: string, events: ConnectionEvents) { probe.events = events; }
    start() {}
    stop() {}
    call(method: string, params: Record<string, unknown>) {
      return new Promise((resolve) => probe.pending.push({ method, params, resolve }));
    }
  },
}));

import { chooseProject, connect, selectProject, totalAttention, useStore } from './store';

afterEach(() => {
  // A debounced refresh still waiting would swallow the next test's.
  if (vi.isFakeTimers()) vi.runOnlyPendingTimers();
  vi.useRealTimers();
  probe.pending = [];
  useStore.setState({ host: null, project: null, snapshot: null, live: {} });
});

const snapshot = (permission: 'full' | 'auto_review'): Snapshot => ({
  seq: 1, project: null, config: null, roles: [], tasks: [], attention: [], quota: [],
  harnesses: [{ harness: 'codex', permission }], live: {},
});

const project = (id: string, extra: Partial<HostSnapshot['projects'][number]> = {}): HostSnapshot['projects'][number] => ({
  id, name: id, data_dir: `/data/${id}`, repo_path: `/repo/${id}`, state: 'running', registered_at: 1, error: null, attention: [], busy: false, ...extra,
});

const host = (...ids: string[]): HostSnapshot => ({ projects: ids.map((id) => project(id)), attention: [], harnesses: [] });

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
  await answer('host', host('p1'));
  expect(useStore.getState().project).toBe('p1');
  expect(pending('snapshot')[0].params).toEqual({ project: 'p1' });
  await answer('snapshot', snapshot('full'));
  expect(probe.pending.find((p) => p.method === 'host')?.params ?? {}).not.toHaveProperty('project');
});

test('a reply about the project the window left is dropped', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  useStore.setState({ host: host('p1', 'p2'), project: 'p1' });
  probe.events!.push({ type: 'events', project: 'p1', events: [{ seq: 2, kind: 'task.updated', entity: 't', payload: {} }] });
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
  probe.events!.push({ type: 'events', project: 'p2', events: [{ seq: 2, kind: 'task.updated', entity: 't', payload: {} }] });
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
  const others = { projects: [failed, archived, project('p3')], attention: [], harnesses: [] };
  expect(chooseProject(host('p1', 'p2'), 'p2', false)).toBe('p2');
  // A project that closes leaves the window on 「全部」.
  expect(chooseProject(others, 'p1', false)).toBe(null);
  expect(chooseProject(others, null, false)).toBe(null);
  // At start: the only open project, else 「全部」.
  expect(chooseProject(others, null, true)).toBe('p3');
  expect(chooseProject(host('p1', 'p2'), null, true)).toBe(null);
  expect(chooseProject({ projects: [failed, archived], attention: [], harnesses: [] }, null, true)).toBe(null);
});

test('the total counts each project’s items and the host’s quota domains once', () => {
  const accept = { kind: 'accept', task_id: 't', title: 'T', verification_id: 'v' } as const;
  const quota = { kind: 'quota', domain: { harness: 'claude', account: '', blocked_at: 1, resets_at: null, message: null } } as const;
  const counted: HostSnapshot = {
    projects: [project('p1', { attention: [accept] }), project('p2', { attention: [accept, accept] })],
    attention: [quota],
    harnesses: [],
  };
  expect(totalAttention(counted)).toBe(4);
  expect(totalAttention(null)).toBe(0);
});
