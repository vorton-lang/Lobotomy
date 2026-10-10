import { afterEach, expect, test, vi } from 'vitest';
import type { ConnectionEvents } from './api/connection';
import type { Snapshot } from './api/types';

const probe = vi.hoisted(() => ({
  events: null as ConnectionEvents | null,
  pending: [] as ((value: unknown) => void)[],
}));

vi.mock('./api/connection', async (original) => ({
  ...(await original<typeof import('./api/connection')>()),
  Connection: class {
    constructor(_url: string, events: ConnectionEvents) { probe.events = events; }
    start() {}
    stop() {}
    call() { return new Promise((resolve) => probe.pending.push(resolve)); }
  },
}));

import { connect, useStore } from './store';

afterEach(() => {
  vi.useRealTimers();
  probe.pending = [];
  useStore.setState({ snapshot: null, live: {} });
});

const snapshot = (permission: 'full' | 'auto_review'): Snapshot => ({
  seq: 1, project: null, config: null, roles: [], tasks: [], attention: [], quota: [],
  harnesses: [{ harness: 'codex', permission }], live: {},
});

test('a superseded snapshot response does not replace the newer snapshot or live view', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  const events = probe.events!;
  events.push({ type: 'host' });
  await vi.advanceTimersByTimeAsync(120);
  events.push({ type: 'host' });
  await vi.advanceTimersByTimeAsync(120);
  expect(probe.pending).toHaveLength(2);

  // Host changes have no project event: both snapshots can have the same event sequence.
  const newer = snapshot('auto_review');
  probe.pending[1](newer);
  await Promise.resolve();
  expect(useStore.getState().snapshot).toBe(newer);
  expect(useStore.getState().live).toBe(newer.live);

  probe.pending[0](snapshot('full'));
  await Promise.resolve();
  expect(useStore.getState().snapshot).toBe(newer);
  expect(useStore.getState().live).toBe(newer.live);
});

test('the first snapshot can initialize while another refresh is still pending', async () => {
  vi.useFakeTimers();
  connect('ws://unused');
  const events = probe.events!;
  events.opened();
  events.push({ type: 'host' });
  await vi.advanceTimersByTimeAsync(120);
  expect(probe.pending).toHaveLength(2);

  const initial = snapshot('full');
  probe.pending[0](initial);
  await Promise.resolve();
  expect(useStore.getState().snapshot).toBe(initial);
  const newer = snapshot('auto_review');
  probe.pending[1](newer);
  await Promise.resolve();
  expect(useStore.getState().snapshot).toBe(newer);
});
