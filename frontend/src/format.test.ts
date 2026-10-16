import { describe, expect, it } from 'vitest';
import type {
  Attempt,
  Attention,
  CommandRecord,
  Failure,
  Item,
  LiveTurn,
  Message,
  RoleView,
  TaskDetail,
  TaskView,
  Turn,
} from './api/types';
import {
  abnormalEnd,
  asReport,
  attachmentExpired,
  attachmentLine,
  attentionKey,
  attentionSummary,
  attentionTask,
  buildRows,
  commandSummary,
  composerHint,
  continueNote,
  deliveryReport,
  firstLines,
  fits,
  lastLines,
  liveSize,
  previewLine,
  quietFor,
  roleActivity,
  roleHue,
  roleLook,
  toolRunning,
  turnLook,
  splitReply,
  textOf,
  timeline,
  toolSubject,
  turnOutcome,
  voidReport,
} from './format';
import { mergePage, type ThreadState } from './store';

const turn = (id: string, input = '【来自 你】\n做点事'): Turn => ({
  id,
  role: 'Malkuth',
  harness: 'codex',
  native_session_id: 'session',
  native_id: null,
  process_start: null,
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
  reply_to: null,
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
      t3: {
        role: 'Malkuth',
        started_at: 5,
        items: { c1: { kind: 'command', content: { command: 'ls', output: null }, started_at: 6 } },
      },
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
        items: [
          call('r1', 1, 't1'),
          item('a1', 2, 't1'),
          call('r2', 3, 't2'),
          item('a2', 4, 't2'),
          item('a3', 5, 't3'),
          call('r3', 6, 't3'),
        ],
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
    expect(continueNote('【来自 Lobotomy】\n上一个 turn 被中断。\n\n【用户的本地时间】2026-10-13 21:05（周二）')).toBe(
      '上一个 turn 被中断。',
    );
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
  done_turn_id: null,
  done_summary: null,
});

const detail = (patch: Partial<TaskDetail>): TaskDetail => ({
  task: {
    id: 't',
    title: 't',
    body: '',
    executor: 'Malkuth',
    phase: 'executing',
    paused: false,
    paused_by: null,
    blocked_reason: null,
    criteria_version: 1,
    queue_pos: null,
    revision: 1,
    created_at: 1,
    closed_at: null,
    assignment: null,
  },
  criteria: [{ version: 1, text: '', created_by: 'user', created_at: 1 }],
  attempts: [],
  decisions: [],
  undelivered_messages: [],
  ...patch,
});

describe('timeline', () => {
  it('puts rounds and decisions in time order, causes first', () => {
    const d = detail({
      attempts: [attempt(1, 10, 20), attempt(2, 40, null)],
      // The send-back and the round it opens share a timestamp.
      decisions: [{ id: 'd1', kind: 'send_back', actor: 'user', detail: { reason: '再改改' }, created_at: 40 }],
    });
    expect(timeline(d).map((e) => e.text)).toEqual([
      '建立任务',
      '第 1 轮执行开始',
      '第 1 轮交出候选成果',
      '你退回了：再改改',
      '第 2 轮执行开始',
    ]);
  });

  it('says which decisions Angela made', () => {
    const d = detail({
      decisions: [{ id: 'd1', kind: 'send_back', actor: 'manager', detail: { reason: '补上行号' }, created_at: 40 }],
    });
    d.task.assignment = { actions: ['read_only'], stop_when: '找到原因就停', interpretation: '先定位' };
    expect(timeline(d).map((e) => e.text)).toEqual(['Angela 派出任务', 'Angela 退回了：补上行号']);
  });
});

describe("Angela's reply", () => {
  it('splits at the references the runtime renders', () => {
    expect(splitReply('改好了。\n[[att:ver_01]]\n你之前说 [[quote:msg_9]] 现在满足')).toEqual([
      { kind: 'text', text: '改好了。\n' },
      { kind: 'att', id: 'ver_01' },
      { kind: 'text', text: '\n你之前说 ' },
      { kind: 'quote', id: 'msg_9' },
      { kind: 'text', text: ' 现在满足' },
    ]);
    expect(splitReply('没有引用')).toEqual([{ kind: 'text', text: '没有引用' }]);
    // A malformed reference stays text.
    expect(splitReply('[[att: x]]')).toEqual([{ kind: 'text', text: '[[att: x]]' }]);
  });

  it('previews the message a reply replies to as plain text', () => {
    expect(previewLine('**alpha · 导出报表**：第 1 轮已通过验证。\n\n[[att:ver_1]]')).toBe(
      'alpha · 导出报表：第 1 轮已通过验证。',
    );
    expect(previewLine('## 结论\n细节')).toBe('结论');
    expect(previewLine('> - 你说过 `--force` 不要用，见 [说明](https://x.y)')).toBe('你说过 --force 不要用，见 说明');
    // Lines of markup alone say nothing; a reference shows as what it is.
    expect(previewLine('---\n```ts\nconst a = 1;\n```')).toBe('const a = 1;');
    expect(previewLine('[[quote:msg_1]] 这句还算数吗')).toBe('（原话） 这句还算数吗');
    expect(previewLine('[[att:ver_1]]')).toBe('（附件）');
    // Underscores inside names stay.
    expect(previewLine('改了 snake_case_name 和 2*3')).toBe('改了 snake_case_name 和 2*3');
    expect(previewLine('长'.repeat(70))).toBe(`${'长'.repeat(60)}…`);
  });

  it('writes what an attachment is and whether it still holds', () => {
    const r = {
      project: 'p1',
      project_name: 'alpha',
      kind: 'report' as const,
      task_id: 't',
      title: '加行号',
      role: 'Malkuth',
      report_title: '完成了',
      status: 'done',
      at: 0,
      chars: 12,
      effect: 'recorded' as const,
    };
    expect(attachmentLine(r)).toContain('alpha · Malkuth 的汇报 · 完成了');
    expect(attachmentExpired(r)).toBeNull();
    expect(attachmentExpired({ ...r, effect: 'late' })).toBe('没有生效');
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
    expect(commandSummary({ blob: 'h', size: 300_000, head: 'echo start\nmore', tail: 'end' })).toEqual({
      line: 'echo start',
      size: '300000 个字符',
    });
  });
});

const role = (patch: Partial<RoleView>): RoleView => ({
  name: 'Malkuth',
  kind: 'worker',
  harness: 'codex',
  model: null,
  effort: null,
  task_id: null,
  unfinished: null,
  last_turn: null,
  hold: null,
  stalled: null,
  queued_messages: 0,
  ...patch,
});

const taskView = (phase: TaskView['phase']): TaskView => ({
  ...detail({}).task,
  title: '写 work.txt',
  phase,
  attempt_seq: 1,
  attempt_open: phase === 'executing',
  undelivered_messages: 0,
  reply_queued: false,
});

describe('roleActivity', () => {
  it('says what keeps the role, the most pressing first', () => {
    const running = { ...turn('t'), state: 'running' as const, outcome: null, started_at: 1_000 };
    expect(roleActivity(role({ unfinished: running }), undefined, 66_000)).toBe('正在跑 turn · 1:05');
    expect(roleActivity(role({ unfinished: { ...running, state: 'unknown' } }), undefined, 0)).toBe(
      '上一个 turn 状态未知',
    );
    const hold = { kind: 'session_unidentified' as const, session_id: 's' };
    expect(
      roleActivity(role({ hold, stalled: { task_id: 't', turn_id: 't', report_error: null } }), undefined, 0),
    ).toBe('停下，等你决定');
    expect(roleActivity(role({}), taskView('accepting'), 0)).toBe('待验收');
    expect(roleActivity(role({}), undefined, 0)).toBe('空闲');
  });

  it('tells a question waiting for the user from a reply waiting for the next turn (#17)', () => {
    const asked = { ...taskView('executing'), blocked_reason: '空值怎么处理？' };
    expect(roleActivity(role({}), asked, 0)).toBe('提出了问题，等你回答');
    // The reply is the user's or Angela's (roles-and-tasks.md §4).
    expect(roleActivity(role({}), { ...asked, reply_queued: true }, 0)).toBe('回复在排队，下一个 turn 交给它');
    const registered = { ...turn('t'), state: 'registered' as const, outcome: null };
    expect(roleActivity(role({ unfinished: registered }), asked, 0)).toBe('即将开始 turn');
  });

  it('has a look for each case: only work in progress runs, only what needs the user waits', () => {
    const running = { ...turn('t'), state: 'running' as const, outcome: null };
    expect(roleLook(role({ unfinished: running }), undefined)).toBe('run');
    expect(roleLook(role({ unfinished: { ...running, state: 'unknown' } }), undefined)).toBe('wait');
    const ended = { ...turn('t'), state: 'ended' as const, outcome: 'failed' as const };
    expect(roleLook(role({ hold: { kind: 'abnormal', turn: ended } }), undefined)).toBe('fail');
    expect(roleLook(role({ hold: { kind: 'session_unidentified', session_id: 's' } }), undefined)).toBe('wait');
    const asked = { ...taskView('executing'), blocked_reason: '空值怎么处理？' };
    expect(roleLook(role({}), asked)).toBe('wait');
    expect(roleLook(role({}), { ...asked, reply_queued: true })).toBe('idle');
    expect(roleLook(role({}), taskView('accepting'))).toBe('wait');
    expect(roleLook(role({}), taskView('executing'))).toBe('run');
    expect(roleLook(role({}), undefined)).toBe('idle');
    expect(turnLook(ended)).toBe('fail');
    expect(turnLook({ ...ended, outcome: 'completed' })).toBe('idle');
  });
});

describe('who a role is and how long its turn is quiet', () => {
  it('gives each named role its color and any other role one by its name', () => {
    expect([roleHue('Angela'), roleHue('Malkuth'), roleHue('Binah'), roleHue('Yesod')]).toEqual([0, 1, 2, 3]);
    const names = ['Hod', 'Netzach', 'Chesed'];
    const hues = names.map(roleHue);
    expect(names.map(roleHue)).toEqual(hues);
    expect(new Set(hues).size).toBeGreaterThan(1); // Six colors allow collisions, but not a constant.
    expect(hues.every((hue) => Number.isInteger(hue) && hue >= 0 && hue < 6)).toBe(true);
  });

  it('says a turn is quiet only after two minutes without anything new', () => {
    expect(quietFor(undefined, 1e9)).toBeNull();
    expect(quietFor(0, 119_000)).toBeNull();
    expect(quietFor(0, 120_000)).toBe('2 分钟没有新输出');
    expect(quietFor(0, 12 * 60_000 + 5_000)).toBe('12 分钟没有新输出');
  });

  it('tells a live item that grew from one that did not, and a running tool', () => {
    const text = (t: string, status?: string) => ({
      kind: 'agent_message' as const,
      started_at: 1,
      content: { text: t, ...(status && { status }) },
    });
    expect(liveSize(text('ab'))).not.toBe(liveSize(text('abc')));
    expect(liveSize(text('ab'))).toBe(liveSize(text('xy')));
    const command = {
      kind: 'command' as const,
      started_at: 1,
      content: { command: 'npm test', output: null, status: 'in_progress' },
    };
    expect(toolRunning({ role: 'Malkuth', started_at: 1, items: { a: text('hi'), b: command } })).toBe(true);
    expect(toolRunning({ role: 'Malkuth', started_at: 1, items: { a: text('hi') } })).toBe(false);
    expect(toolRunning(undefined)).toBe(false);
    const dynamic = { kind: 'future_kind', started_at: 1, content: null };
    expect(toolRunning({ role: 'Malkuth', started_at: 1, items: { a: dynamic } })).toBe(false);
    expect(liveSize(dynamic)).toBe(liveSize({ ...dynamic, content: {} }));
  });
});

describe('toolSubject', () => {
  it('says what a tool of the harness works on, in one line', () => {
    expect(toolSubject({ file_path: 'C:/work/src/lib.rs' })).toBe('C:/work/src/lib.rs');
    expect(toolSubject({ pattern: 'fn main', path: 'src' })).toBe('fn main');
    expect(toolSubject({ path: 'src' })).toBe('src');
    expect(toolSubject({ subject: 'First item', description: 'longer' })).toBe('First item');
    expect(toolSubject({ query: 'select:TaskCreate', max_results: 1 })).toBe('select:TaskCreate');
    expect(toolSubject({ prompt: `first line\nsecond line` })).toBe('first line');
    expect(toolSubject({ prompt: 'x'.repeat(200) })).toBe(`${'x'.repeat(120)}…`);
    expect(toolSubject({ taskId: '1', status: 'completed' })).toBe('');
    expect(toolSubject(null)).toBe('');
  });
});

describe('composerHint', () => {
  it('says where a message goes, naming the role', () => {
    expect(composerHint('Yesod', undefined)).toBe('发给 Yesod（不属于任何任务）');
    expect(composerHint('Yesod', taskView('executing'))).toBe('发给 Yesod · 任务「写 work.txt」');
    expect(composerHint('Yesod', taskView('accepting'))).toContain('退回后交给 Yesod；直接验收则不再投递');
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

describe('abnormalEnd', () => {
  const failed = (failure: Partial<Failure> | null, outcome: Turn['outcome'] = 'failed'): Turn => ({
    ...turn('t'),
    outcome,
    failure: failure && { unstarted: false, kind: 'other', message: 'x', resets_at: null, ...failure },
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
    const blocked = (task_id: string, reason = 'r'): Attention => ({
      kind: 'task_blocked',
      task_id,
      title: 't',
      reason,
    });
    expect(attentionKey(blocked('a'))).toBe(attentionKey(blocked('a', '换了个问法')));
    expect(attentionKey(blocked('a'))).not.toBe(attentionKey(blocked('b')));
    const accept: Attention = { kind: 'accept', task_id: 'a', title: 't' };
    expect(attentionKey(accept)).not.toBe(attentionKey(blocked('a')));
  });
});

describe('attentionSummary', () => {
  it('says in one line what waits, and names the task going there opens', () => {
    const accept: Attention = { kind: 'accept', task_id: 'a', title: '导出' };
    expect(attentionSummary(accept)).toBe('「导出」交出了候选成果，等你验收');
    expect(attentionTask(accept)).toBe('a');
    const failed: Attention = {
      kind: 'hold',
      role: 'Malkuth',
      hold: { kind: 'abnormal', turn: { ...turn('t1'), outcome: 'interrupted' } },
    };
    expect(attentionSummary(failed)).toBe('Malkuth 的上一个 turn 被中断，停下等你决定');
    expect(attentionTask(failed)).toBe(null);
  });
});

describe('deliveryReport', () => {
  const first = { ...attempt(1, 10, 20), done_summary: 'First delivery' };
  const second = { ...attempt(2, 30, null), done_summary: 'Second delivery' };

  it('shows the newest report without calling it handed in before its turn ends', () => {
    expect(deliveryReport([first, second])).toEqual({
      text: 'Second delivery',
      round: 2,
      earlier: false,
      pending: true,
    });
    expect(deliveryReport([first, { ...second, end_reason: 'candidate' as const }])?.pending).toBe(false);
  });

  it('labels a previous report as old while the latest round has none', () => {
    expect(deliveryReport([first, attempt(2, 30, null)])).toEqual({
      text: 'First delivery',
      round: 1,
      earlier: true,
      pending: false,
    });
  });

  it('handles missing and empty reports without inventing a delivery', () => {
    expect(deliveryReport([])).toBeNull();
    expect(deliveryReport([attempt(1, 10, null)])).toBeNull();
    expect(deliveryReport([{ ...first, done_summary: '  ' }])).toBeNull();
  });

  it('does not retain an invalidated report when the backend clears it', () => {
    expect(deliveryReport([second])?.text).toBe('Second delivery');
    expect(deliveryReport([{ ...second, done_summary: null }])).toBeNull();
    expect(deliveryReport([first, { ...second, done_summary: null }])?.earlier).toBe(true);
  });
});
