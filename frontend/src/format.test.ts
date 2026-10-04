import { describe, expect, it } from 'vitest';
import type { Item, Message, Turn } from './api/types';
import { buildRows, preview } from './format';
import { mergePage, type ThreadState } from './store';

const turn = (id: string, input = '【来自 你】\n做点事'): Turn => ({
  id,
  role: 'Malkuth',
  task_id: null,
  attempt_id: null,
  input,
  state: 'ended',
  outcome: 'completed',
  failure: null,
  pid: 1,
  done_at: null,
  registered_at: 1,
  started_at: 1,
  ended_at: 2,
});

const item = (id: string, seq: number, turnId: string, kind = 'agent_message'): Item => ({
  id,
  seq,
  turn_id: turnId,
  kind,
  content: kind === 'input' ? { source: 'turn.input' } : { text: id },
  command_id: null,
  created_at: seq,
});

const message = (id: string, turnId: string | null): Message => ({
  id,
  source: 'user',
  task_id: null,
  body: id,
  turn_id: turnId,
  created_at: 1,
});

const thread = (patch: Partial<ThreadState>): ThreadState => ({
  role: 'Malkuth',
  items: [],
  turns: {},
  messages: [],
  commands: {},
  queued: [],
  hasMore: false,
  loaded: true,
  ...patch,
});

describe('buildRows', () => {
  it('opens each turn with its messages and skips the input item', () => {
    const rows = buildRows(
      thread({
        items: [item('i1', 1, 't1', 'input'), item('i2', 2, 't1'), item('i3', 3, 't2', 'input'), item('i4', 4, 't2')],
        turns: { t1: turn('t1'), t2: turn('t2', '【来自 Lobotomy】\n上一个 turn 失败。') },
        messages: [message('m1', 't1')],
        queued: [message('m9', null)],
      }),
      {},
    );
    expect(rows.map((r) => r.kind)).toEqual(['turn', 'message', 'item', 'turn', 'input', 'item', 'queued']);
  });

  it('shows the items of a running turn, or that it runs', () => {
    const live = {
      t3: { role: 'Malkuth', started_at: 5, items: { c1: { kind: 'command', content: { command: 'ls' }, started_at: 6 } } },
      t4: { role: 'Yesod', started_at: 5, items: {} },
    };
    expect(buildRows(thread({}), live).map((r) => r.kind)).toEqual(['live']);
    expect(buildRows(thread({}), { t3: { ...live.t3, items: {} } }).map((r) => r.kind)).toEqual(['running']);
  });
});

describe('mergePage', () => {
  const page = (items: Item[]) => ({ items, has_more: true, turns: {}, messages: [], commands: {}, queued: [] });
  it('appends newer items once and keeps older pages in front', () => {
    let t = mergePage(thread({}), page([item('a', 5, 't')]), 'replace');
    t = mergePage(t, page([item('a', 5, 't'), item('b', 6, 't')]), 'newer');
    t = mergePage(t, page([item('z', 4, 't')]), 'older');
    expect(t.items.map((i) => i.id)).toEqual(['z', 'a', 'b']);
  });
});

describe('preview', () => {
  it('shows head and tail of a field in the blob store', () => {
    const { text, blob } = preview({ blob: 'h', size: 300_000, head: 'start', tail: 'end' });
    expect(blob?.blob).toBe('h');
    expect(text.startsWith('start') && text.endsWith('end')).toBe(true);
  });
});
