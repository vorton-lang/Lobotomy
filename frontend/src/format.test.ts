import { describe, expect, it } from 'vitest';
import type { Attempt, Capture, Item, Message, TaskDetail, Turn, VerificationRow } from './api/types';
import { buildRows, continueNote, firstLines, fits, lastLines, preview, timeline, workRange } from './format';
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

describe('continueNote', () => {
  it('is the note in front of the queued messages, which show on their own', () => {
    const input = '【来自 Lobotomy】\n上一个 turn 失败。请先检查。\n\n【来自 用户 · 任务 task_1】\n换个做法';
    expect(continueNote(input)).toBe('上一个 turn 失败。请先检查。');
    expect(continueNote('【来自 Lobotomy】\n上一个 turn 被中断。')).toBe('上一个 turn 被中断。');
    expect(continueNote('【来自 Lobotomy · 任务 task_1】\n任务：x（第 1 轮执行）')).toBeNull();
  });
});

const attempt = (seq: number, started: number, ended: number | null): Attempt => ({
  id: `att${seq}`,
  seq,
  started_at: started,
  ended_at: ended,
  end_reason: ended ? 'candidate' : null,
  candidate_id: null,
});

const verification = (seq: number, at: number, state: VerificationRow['state']): VerificationRow => ({
  id: `ver${seq}`,
  attempt_id: `att${seq}`,
  capture_id: `cap${seq}`,
  base: 'base0000',
  config_version: 1,
  commit_id: `cand${seq}`,
  conflicts: [],
  state,
  created_at: at,
  finished_at: state === 'running' ? null : at + 1,
});

const detail = (patch: Partial<TaskDetail>): TaskDetail => ({
  task: { id: 't', title: 't', body: '', executor: 'Malkuth', phase: 'executing', paused: false, blocked_reason: null, criteria_version: 1, queue_pos: null, revision: 1, created_at: 1, closed_at: null },
  criteria: [{ version: 1, text: '', created_by: 'user', created_at: 1 }],
  attempts: [],
  captures: [],
  verifications: [],
  checks: [],
  decisions: [],
  publications: [],
  ...patch,
});

describe('timeline', () => {
  it('puts rounds, verifications and decisions in time order, causes first', () => {
    const d = detail({
      attempts: [attempt(1, 10, 20), attempt(2, 40, null)],
      verifications: [verification(1, 25, 'passed')],
      // The send-back and the round it opens share a timestamp.
      decisions: [{ id: 'd1', kind: 'send_back', actor: 'user', detail: { reason: '再改改' }, created_at: 40 }],
    });
    expect(timeline(d).map((e) => e.text)).toEqual([
      '建立任务',
      '第 1 轮执行开始',
      '第 1 轮交出候选成果',
      '第 1 轮的候选成果开始验证，基于集成版本 base0000',
      '第 1 轮的验证通过',
      '你退回了：再改改',
      '第 2 轮执行开始',
    ]);
  });
});

describe('workRange', () => {
  it('marks the last round’s candidate as earlier while the next round has nothing yet', () => {
    const d = detail({ attempts: [attempt(1, 10, 20), attempt(2, 40, null)], verifications: [verification(1, 25, 'passed')] });
    expect(workRange(d)).toMatchObject({ to: 'cand1', earlier: true, label: '上一轮（第 1 轮）的候选成果' });
  });

  it('shows the latest round’s own work once there is some', () => {
    const capture: Capture = {
      id: 'c2', turn_id: 'turn2', role: 'Malkuth', task_id: 't', attempt_id: 'att2', kind: 'turn',
      base: 'base0000', state: 'pinned', commit_id: 'work2', detail: null, created_at: 50,
    };
    const d = detail({ attempts: [attempt(1, 10, 20), attempt(2, 40, null)], verifications: [verification(1, 25, 'passed')], captures: [capture] });
    expect(workRange(d)).toMatchObject({ to: 'work2', earlier: false });
  });
});

describe('output clipping', () => {
  it('keeps the first and last lines of a long text', () => {
    const text = Array.from({ length: 100 }, (_, i) => `line ${i}`).join('\n');
    expect(fits(text)).toBe(false);
    expect(firstLines(text).split('\n')).toEqual(['line 0', 'line 1', 'line 2', 'line 3', 'line 4', 'line 5']);
    expect(lastLines(text).split('\n').at(-1)).toBe('line 99');
    expect(fits('short\ntext')).toBe(true);
  });
});

describe('preview', () => {
  it('shows head and tail of a field in the blob store', () => {
    const { text, blob } = preview({ blob: 'h', size: 300_000, head: 'start', tail: 'end' });
    expect(blob?.blob).toBe('h');
    expect(text.startsWith('start') && text.endsWith('end')).toBe(true);
  });
});
