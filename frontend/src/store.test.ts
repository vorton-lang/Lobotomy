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

import { chooseProject, connect, useStore } from './store';

afterEach(() => {
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
  await answer('host', host('p1', 'p2'));
  expect(useStore.getState().project).toBe('p1');
  expect(pending('snapshot')[0].params).toEqual({ project: 'p1' });
  await answer('snapshot', snapshot('full'));
  expect(probe.pending.find((p) => p.method === 'host')?.params ?? {}).not.toHaveProperty('project');
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

test('the window keeps its project while it stays open, else takes the first open one', () => {
  const failed = project('p1', { error: '数据目录不存在' });
  const archived = project('p2', { state: 'archived' });
  expect(chooseProject(host('p1', 'p2'), 'p2')).toBe('p2');
  expect(chooseProject({ projects: [failed, archived, project('p3')], attention: [], harnesses: [] }, 'p1')).toBe('p3');
  expect(chooseProject({ projects: [failed, archived], attention: [], harnesses: [] }, null)).toBe(null);
});
