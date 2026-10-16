import { describe, expect, it } from 'vitest';
import type { Attempt, Attention, Failure, RoleView, TaskDetail, TaskView, Turn } from './api/types';
import {
  abnormalEnd,
  attentionKey,
  continueNote,
  firstLines,
  fits,
  lastLines,
  previewLine,
  roleActivity,
  timeline,
  turnOutcome,
} from './format';

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
});

describe("Angela's reply", () => {
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
  it('tells a question waiting for the user from a reply waiting for the next turn (#17)', () => {
    const asked = { ...taskView('executing'), blocked_reason: '空值怎么处理？' };
    expect(roleActivity(role({}), asked, 0)).toBe('提出了问题，等你回答');
    // The reply is the user's or Angela's (roles-and-tasks.md §4).
    expect(roleActivity(role({}), { ...asked, reply_queued: true }, 0)).toBe('回复在排队，下一个 turn 交给它');
    const registered = { ...turn('t'), state: 'registered' as const, outcome: null };
    expect(roleActivity(role({ unfinished: registered }), asked, 0)).toBe('即将开始 turn');
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
