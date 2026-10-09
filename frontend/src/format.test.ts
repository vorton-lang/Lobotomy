import { describe, expect, it } from 'vitest';
import type {
  Attempt,
  Attention,
  Capture,
  CheckRun,
  CommandRecord,
  Failure,
  Item,
  LiveTurn,
  Message,
  RoleView,
  TaskDetail,
  TaskView,
  Turn,
  Verification,
} from './api/types';
import {
  abnormalEnd,
  asReport,
  attentionKey,
  buildRows,
  checkSummary,
  commandSummary,
  composerHint,
  continueNote,
  firstLines,
  fits,
  lastLines,
  roleActivity,
  stoppedPaths,
  textOf,
  timeline,
  turnOutcome,
  voidReport,
  workRange,
} from './format';
import { mergePage, type ThreadState } from './store';

const turn = (id: string, input = '【来自 你】\n做点事'): Turn => ({
  id,
  role: 'Malkuth',
  harness: 'codex',
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

const item = (id: string, seq: number, turnId: string, kind: 'input' | 'agent_message' = 'agent_message'): Item => ({
  id,
  seq,
  turn_id: turnId,
  ...(kind === 'input' ? { kind, content: { source: 'turn.input' } } : { kind, content: { text: id } }),
  command_id: null,
  created_at: seq,
});

const message = (id: string, turnId: string | null): Message => ({
  id,
  seq: 1,
  role: 'Malkuth',
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
    const live: Record<string, LiveTurn> = {
      t3: { role: 'Malkuth', started_at: 5, items: { c1: { kind: 'command', content: { command: 'ls', output: null }, started_at: 6 } } },
      t4: { role: 'Yesod', started_at: 5, items: {} },
    };
    expect(buildRows(thread({}), live).map((r) => r.kind)).toEqual(['live']);
    const running = buildRows(thread({}), { t3: { ...live.t3, items: {} } });
    expect(running).toMatchObject([{ kind: 'running', role: 'Malkuth' }]);
  });

  it('folds the body of a done report when a later message of its turn follows (#18)', () => {
    const call = (id: string, seq: number, turnId: string): Item => ({
      id,
      seq,
      turn_id: turnId,
      kind: 'mcp_call',
      content: { server: 'lobotomy', tool: 'org_report', arguments: {}, result_text: null, error_text: null },
      command_id: `cmd-${id}`,
      created_at: seq,
    });
    const report = (status: 'done' | 'progress'): CommandRecord => ({
      name: 'org_report',
      args: { title: 't', body: 'b', status, blocked_on: null },
      result: 'recorded',
    });
    const rows = buildRows(
      thread({
        // Followed by a message; a progress report; a done report that ends its turn.
        items: [call('r1', 1, 't1'), item('a1', 2, 't1'), call('r2', 3, 't2'), item('a2', 4, 't2'), item('a3', 5, 't3'), call('r3', 6, 't3')],
        commands: { 'cmd-r1': report('done'), 'cmd-r2': report('progress'), 'cmd-r3': report('done') },
      }),
      {},
    );
    const fold = Object.fromEntries(rows.flatMap((r) => (r.kind === 'item' ? [[r.item.id, r.fold]] : [])));
    expect(fold).toMatchObject({ r1: true, r2: false, r3: false, a1: false });
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
  task_id: 't',
  seq,
  started_at: started,
  ended_at: ended,
  end_reason: ended ? 'candidate' : null,
  code_start: null,
  done_turn_id: null,
  done_summary: null,
  candidate_id: null,
  conflicts: [],
});

const verification = (seq: number, at: number, state: Verification['state']): Verification => ({
  id: `ver${seq}`,
  task_id: 't',
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
  task: { id: 't', title: 't', body: '', executor: 'Malkuth', phase: 'executing', paused: false, blocked_reason: null, criteria_version: 1, queue_pos: null, revision: 1, created_at: 1, closed_at: null, origin_capture: null },
  criteria: [{ version: 1, text: '', created_by: 'user', created_at: 1 }],
  attempts: [],
  captures: [],
  verifications: [],
  checks: [],
  decisions: [],
  publications: [],
  undelivered_messages: [],
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
      base: 'base0000', state: 'pinned', commit_id: 'work2', detail: null, created_at: 50, outside: null,
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

describe('textOf', () => {
  it('shows head and tail of a field in the blob store', () => {
    const text = textOf({ blob: 'h', size: 300_000, head: 'start', tail: 'end' });
    expect(text.startsWith('start') && text.endsWith('end')).toBe(true);
    expect(textOf('whole')).toBe('whole');
    expect(textOf(null)).toBe('');
  });
});

describe('commandSummary', () => {
  it('shows a short command whole and a long one by its first line and size', () => {
    expect(commandSummary('ls -la')).toEqual({ line: 'ls -la', size: null });
    const heredoc = ["cat > a.txt <<'EOF'", 'x', 'EOF'].join('\n');
    expect(commandSummary(heredoc)).toEqual({ line: "cat > a.txt <<'EOF'", size: `共 3 行，${heredoc.length} 个字符` });
    const wide = commandSummary('x'.repeat(200));
    expect(wide.line).toBe(`${'x'.repeat(120)}…`);
    expect(commandSummary({ blob: 'h', size: 300_000, head: 'echo start\nmore', tail: 'end' })).toEqual({ line: 'echo start', size: '300000 个字符' });
  });
});

const role = (patch: Partial<RoleView>): RoleView => ({
  name: 'Malkuth',
  kind: 'worker',
  harness: 'codex',
  model: null,
  slot: 'worker',
  task_id: null,
  unfinished: null,
  last_turn: null,
  hold: null,
  stalled: null,
  outside: null,
  workspace: null,
  queued_messages: 0,
  ...patch,
});

const taskView = (phase: TaskView['phase']): TaskView => ({
  ...detail({}).task,
  title: '写 work.txt',
  phase,
  attempt_seq: 1,
  attempt_open: phase === 'executing',
  verification: null,
  undelivered_messages: 0,
});

describe('roleActivity', () => {
  it('says what keeps the role, the most pressing first', () => {
    const running = { ...turn('t'), state: 'running' as const, outcome: null, started_at: 1_000 };
    expect(roleActivity(role({ unfinished: running }), undefined, 66_000)).toBe('正在跑 turn · 1:05');
    expect(roleActivity(role({ unfinished: { ...running, state: 'unknown' } }), undefined, 0)).toBe('上一个 turn 状态未知');
    const hold = { kind: 'session_unidentified' as const, session_id: 's' };
    expect(roleActivity(role({ hold, stalled: { task_id: 't', turn_id: 't', report_error: null } }), undefined, 0)).toBe('停下，等你决定');
    expect(roleActivity(role({}), taskView('verifying'), 0)).toBe('验证中');
    expect(roleActivity(role({}), undefined, 0)).toBe('空闲');
  });

  it('tells a question waiting for the user from a reply waiting for the next turn (#17)', () => {
    const asked = { ...taskView('executing'), blocked_reason: '空值怎么处理？' };
    expect(roleActivity(role({}), asked, 0)).toBe('提出了问题，等你回答');
    expect(roleActivity(role({}), { ...asked, undelivered_messages: 1 }, 0)).toBe('你的回复在排队，下一个 turn 交给它');
    const registered = { ...turn('t'), state: 'registered' as const, outcome: null };
    expect(roleActivity(role({ unfinished: registered }), asked, 0)).toBe('即将开始 turn');
  });
});

describe('composerHint', () => {
  it('says where a message goes, naming the role', () => {
    expect(composerHint('Yesod', undefined)).toBe('发给 Yesod（不属于任何任务）');
    expect(composerHint('Yesod', taskView('executing'))).toBe('发给 Yesod · 任务「写 work.txt」');
    expect(composerHint('Yesod', taskView('accepting'))).toContain('退回后交给 Yesod；直接验收则不再投递');
  });
});

describe('checkSummary', () => {
  const check = (patch: Partial<CheckRun>): CheckRun => ({
    id: 'c',
    verification_id: 'v',
    seq: 0,
    command: 'cargo test',
    exit_code: 0,
    timed_out: false,
    output: {},
    duration_ms: 1500,
    ...patch,
  });
  it('gives the exit code or the timeout, and the time', () => {
    expect(checkSummary(check({}))).toBe('退出码 0 · 1.5 秒');
    expect(checkSummary(check({ exit_code: null, timed_out: true }))).toBe('超时 · 1.5 秒');
  });
});

describe('reports', () => {
  const record = (result: string): CommandRecord => ({
    name: 'org_report',
    args: { title: 't', body: 'b', status: 'done', blocked_on: null },
    result,
  });
  it('tells a report that took effect from one that did not', () => {
    expect(voidReport(asReport(record('recorded'))!)).toBeNull();
    expect(voidReport(asReport(record('no_task'))!)).toContain('不属于任何任务');
    expect(asReport({ name: 'other_tool', args: {}, result: null })).toBeNull();
  });
});

describe('stoppedPaths', () => {
  it('lists the new files over the guardrail with their size, or what cannot be captured', () => {
    const capture: Capture = {
      id: 'c', turn_id: 't', role: 'Malkuth', task_id: null, attempt_id: null, kind: 'turn', base: 'b',
      state: 'oversized', commit_id: null, detail: { files: [{ path: 'big.bin', size: 2048 }], total_bytes: 2048 },
      created_at: 1, outside: null,
    };
    expect(stoppedPaths(capture)).toEqual(['big.bin（2.0 KB）']);
    const uncovered = { ...capture, state: 'uncovered' as const, detail: { paths: [{ path: 'vendor/x', kind: 'nested_repo' }] } };
    expect(stoppedPaths(uncovered)).toEqual(['vendor/x（nested_repo）']);
  });
});

describe('abnormalEnd', () => {
  const failed = (failure: Partial<Failure> | null, outcome: Turn['outcome'] = 'failed'): Turn => ({
    ...turn('t'),
    outcome,
    failure: failure && { kind: 'other', message: 'x', resets_at: null, ...failure },
  });

  it('names each way a turn ends abnormally, as the thread does', () => {
    const cases: [Turn, string][] = [
      [failed(null, 'interrupted'), '被中断'],
      [failed({ kind: 'permission', unstarted: true }), '没能启动（权限模式不被允许）'],
      [failed({ unstarted: true }), '没能启动'],
      [failed({ kind: 'quota' }), '因额度不足而失败'],
      [failed({}), '失败'],
    ];
    for (const [t, label] of cases) {
      expect(abnormalEnd(t)).toBe(label);
      expect(turnOutcome(t)).toBe(label);
    }
  });
});

describe('attentionKey', () => {
  it('tells cards apart and keeps a card its key from one snapshot to the next', () => {
    const blocked = (task_id: string, reason = 'r'): Attention => ({ kind: 'task_blocked', task_id, title: 't', reason });
    expect(attentionKey(blocked('a'))).toBe(attentionKey(blocked('a', '换了个问法')));
    expect(attentionKey(blocked('a'))).not.toBe(attentionKey(blocked('b')));
    const accept: Attention = { kind: 'accept', task_id: 'a', title: 't', verification_id: 'v' };
    expect(attentionKey(accept)).not.toBe(attentionKey(blocked('a')));
  });
});
