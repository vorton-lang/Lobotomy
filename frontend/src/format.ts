// Labels and pure transformations for display.

import type { Attention, BlobRef, Item, LiveItem, LiveTurn, Message, Permission, Phase, TaskDetail, Turn } from './api/types';
import type { ThreadState } from './store';

export const PHASE_LABEL: Record<Phase, string> = {
  queued: '排队',
  executing: '执行中',
  verifying: '验证中',
  accepting: '待验收',
  done: '已完成',
  abandoned: '已放弃',
};

export function clock(ms: number | null | undefined): string {
  if (!ms) return '';
  const d = new Date(ms);
  const today = new Date();
  const time = d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  return d.toDateString() === today.toDateString() ? time : `${d.getMonth() + 1}-${d.getDate()} ${time}`;
}

export function elapsed(fromMs: number, nowMs: number): string {
  const s = Math.max(0, Math.floor((nowMs - fromMs) / 1000));
  const m = Math.floor(s / 60);
  return m > 0 ? `${m}:${String(s % 60).padStart(2, '0')}` : `${s} 秒`;
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function shortSha(commit: string | null | undefined): string {
  return commit ? commit.slice(0, 8) : '';
}

export function isBlob(value: unknown): value is BlobRef {
  return typeof value === 'object' && value !== null && 'blob' in value;
}

/** Text of a field that may have gone to the blob store: the full text, or head and tail. */
export function preview(value: unknown): { text: string; blob: BlobRef | null } {
  if (isBlob(value)) return { text: `${value.head}\n…（共 ${bytes(value.size)}，已省略中间部分）…\n${value.tail}`, blob: value };
  return { text: typeof value === 'string' ? value : value == null ? '' : JSON.stringify(value, null, 2), blob: null };
}

/** Lines and characters an output shows before the rest goes behind "查看全文". */
export const SHOWN = { head: 6, tail: 18, chars: 3000 };

export function firstLines(text: string, lines = SHOWN.head, chars = SHOWN.chars / 4): string {
  const head = text.split('\n').slice(0, lines).join('\n');
  return head.length > chars ? `${head.slice(0, chars)}…` : head;
}

export function lastLines(text: string, lines = SHOWN.tail, chars = SHOWN.chars): string {
  const tail = text.split('\n').slice(-lines).join('\n');
  return tail.length > chars ? `…${tail.slice(-chars)}` : tail;
}

/** Whether a text is short enough to show whole. */
export function fits(text: string): boolean {
  return text.length <= SHOWN.chars && text.split('\n').length <= SHOWN.head + SHOWN.tail;
}

export function sourceLabel(source: string): string {
  if (source === 'user') return '你';
  if (source === 'runtime') return 'Lobotomy';
  return source.replace(/^role:/, '');
}

/** A turn's end says nothing about its task: a turn can end normally with the task unfinished. */
export function turnOutcome(turn: Turn): string {
  if (turn.state === 'registered') return '等待启动';
  if (turn.state === 'running') return '运行中';
  if (turn.state === 'unknown') return '状态未知';
  if (turn.outcome === 'completed') return 'turn 正常结束';
  return abnormalEnd(turn);
}

/**
 * How a turn that did not end normally ended. The thread and the card that waits for the user
 * say it in the same words (#16).
 */
export function abnormalEnd(turn: Turn): string {
  if (turn.outcome === 'interrupted') return '被中断';
  if (turn.failure?.kind === 'permission') return '没能启动（权限模式不被允许）';
  if (turn.failure?.unstarted) return '没能启动';
  if (turn.failure?.kind === 'quota') return '因额度不足而失败';
  return '失败';
}

/** A stable key for a card that waits for the user: a card keeps its draft while others come and go (#16). */
export function attentionKey(a: Attention): string {
  switch (a.kind) {
    case 'hold':
      return `hold:${a.role}:${a.hold.kind}`;
    case 'outside_changes':
      return `outside:${a.capture.id}`;
    case 'unknown_turn':
      return `unknown:${a.turn_id}`;
    case 'task_blocked':
      return `blocked:${a.task_id}`;
    case 'stalled':
      return `stalled:${a.turn_id}`;
    case 'accept':
      return `accept:${a.verification_id}`;
    case 'quota':
      return `quota:${a.domain.harness}`;
    case 'job_failed':
      return `job:${a.key}`;
    case 'preview_stopped':
      return 'preview';
  }
}

export const HARNESS_LABEL: Record<string, string> = { codex: 'Codex', claude: 'Claude' };

export const PERMISSION_LABEL: Record<Permission, string> = { full: '完全放开', auto_review: '自动审批' };

/**
 * The note a continue put in front of the turn's input, without the queued messages after it;
 * those show as messages of their own. The format is the backend's (`register` in turn.rs).
 */
export function continueNote(input: string): string | null {
  const head = '【来自 Lobotomy】\n';
  if (!input.startsWith(head)) return null;
  const rest = input.slice(head.length);
  const end = rest.indexOf('\n\n【来自 ');
  return end < 0 ? rest : rest.slice(0, end);
}

export type Row =
  | { key: string; kind: 'turn'; turn: Turn }
  | { key: string; kind: 'message'; message: Message }
  | { key: string; kind: 'input'; turn: Turn; note: string }
  | { key: string; kind: 'item'; item: Item }
  | { key: string; kind: 'live'; turnId: string; itemId: string; item: LiveItem }
  | { key: string; kind: 'running'; turnId: string; since: number }
  | { key: string; kind: 'queued'; message: Message };

/**
 * The thread as rows: each turn opens with a divider and the messages it delivered, then its
 * items. Items of turns still running follow, then messages waiting for a turn.
 */
export function buildRows(thread: ThreadState, live: Record<string, LiveTurn>): Row[] {
  const rows: Row[] = [];
  const byTurn = new Map<string, Message[]>();
  for (const m of thread.messages) {
    if (!m.turn_id) continue;
    byTurn.set(m.turn_id, [...(byTurn.get(m.turn_id) ?? []), m]);
  }
  const shown = new Set<string>();
  for (const item of thread.items) {
    if (!shown.has(item.turn_id)) {
      shown.add(item.turn_id);
      const turn = thread.turns[item.turn_id];
      if (turn) {
        rows.push({ key: `turn-${turn.id}`, kind: 'turn', turn });
        // A continue note is not a message; the turn's input carries it, before the messages.
        const note = continueNote(turn.input);
        if (note !== null) rows.push({ key: `input-${turn.id}`, kind: 'input', turn, note });
        for (const message of byTurn.get(turn.id) ?? []) rows.push({ key: `msg-${message.id}`, kind: 'message', message });
      }
    }
    if (item.kind === 'input') continue;
    rows.push({ key: `item-${item.id}`, kind: 'item', item });
  }
  for (const [turnId, turn] of Object.entries(live)) {
    if (turn.role !== thread.role) continue;
    const items = Object.entries(turn.items);
    if (items.length === 0) rows.push({ key: `running-${turnId}`, kind: 'running', turnId, since: turn.started_at });
    for (const [itemId, item] of items) rows.push({ key: `live-${turnId}-${itemId}`, kind: 'live', turnId, itemId, item });
  }
  for (const message of thread.queued) rows.push({ key: `queued-${message.id}`, kind: 'queued', message });
  return rows;
}

const DECISION_LABEL: Record<string, string> = {
  accept: '你验收了',
  send_back: '你退回了',
  abandon: '放弃了任务',
  reopen: '你重开了任务',
  approve_new_files: '你放行了新增文件',
  discard_uncaptured: '你丢弃了未能采集的内容',
  adopt_outside_changes: '你用任务之外的改动建立了这个任务',
};

/** What a closing decision let go: the user's messages the executor never got (#14). */
function droppedNote(detail: Record<string, unknown>): string {
  const dropped = Array.isArray(detail.dropped_messages) ? (detail.dropped_messages as { body: string }[]) : [];
  if (dropped.length === 0) return '';
  const quoted = dropped.map((m) => `「${m.body.length > 40 ? `${m.body.slice(0, 40)}…` : m.body}」`).join('');
  return `；${dropped.length} 条消息没有投递：${quoted}`;
}

export interface TimelineEntry {
  at: number;
  text: string;
}

/**
 * A task's history in time order: execution rounds, verifications, decisions, publications and
 * criteria changes (#13). What one command did shares a timestamp; the rank puts its cause first,
 * e.g. a send-back before the round it opens.
 */
export function timeline(detail: TaskDetail): TimelineEntry[] {
  const entries: (TimelineEntry & { rank: number })[] = [];
  const add = (at: number | null, rank: number, text: string) => at && entries.push({ at, rank, text });
  const round = (attemptId: string) => detail.attempts.find((a) => a.id === attemptId)?.seq;
  add(detail.task.created_at, 0, '建立任务');
  for (const c of detail.criteria) if (c.version > 1) add(c.created_at, 0, `完成条件改为第 ${c.version} 版`);
  for (const d of detail.decisions) {
    const reason = typeof d.detail.reason === 'string' && d.detail.reason ? `：${d.detail.reason}` : '';
    add(d.created_at, 0, `${DECISION_LABEL[d.kind] ?? d.kind}${reason}${droppedNote(d.detail)}`);
  }
  for (const v of detail.verifications) {
    add(v.created_at, 0, `第 ${round(v.attempt_id)} 轮的候选成果开始验证，基于集成版本 ${shortSha(v.base)}`);
    if (v.state !== 'running') {
      const result = v.state === 'passed' ? '通过' : v.conflicts?.length ? '未通过：有冲突' : '未通过';
      add(v.finished_at, 0, `第 ${round(v.attempt_id)} 轮的验证${result}`);
    }
  }
  for (const a of detail.attempts) {
    add(a.started_at, 2, `第 ${a.seq} 轮执行开始`);
    add(a.ended_at, 1, a.end_reason === 'candidate' ? `第 ${a.seq} 轮交出候选成果` : `第 ${a.seq} 轮结束`);
  }
  for (const p of detail.publications) add(p.created_at, 3, `发布为集成版本 ${shortSha(p.commit_id)}`);
  return entries.sort((a, b) => a.at - b.at || a.rank - b.rank).map(({ at, text }) => ({ at, text }));
}

export interface WorkRange {
  from: string;
  to: string;
  label: string;
  /** The range belongs to an earlier round than the latest one. */
  earlier: boolean;
}

/**
 * The changes to show for a task: the latest round's verified candidate, its candidate, or its
 * latest capture; failing all of these, an earlier round's verified candidate, marked as such
 * (#13).
 */
export function workRange(detail: TaskDetail): WorkRange | null {
  const latest = detail.attempts.at(-1);
  const verification = detail.verifications.at(-1);
  const done = verification?.commit_id && verification.state !== 'running' ? verification : null;
  if (latest && done?.attempt_id === latest.id) {
    return { from: done.base, to: done.commit_id!, label: `第 ${latest.seq} 轮的候选成果，已合到集成版本 ${shortSha(done.base)} 上`, earlier: false };
  }
  const captures = detail.captures.filter((c) => c.commit_id && latest && c.attempt_id === latest.id);
  const candidate = captures.filter((c) => c.kind === 'candidate').at(-1);
  if (latest && candidate) return { from: candidate.base, to: candidate.commit_id!, label: `第 ${latest.seq} 轮的候选成果`, earlier: false };
  const last = captures.at(-1);
  if (latest && last) {
    return { from: last.base, to: last.commit_id!, label: `第 ${latest.seq} 轮目前的工作（${clock(last.created_at)} 采集），还不是候选成果`, earlier: false };
  }
  if (done) {
    const seq = detail.attempts.find((a) => a.id === done.attempt_id)?.seq;
    return { from: done.base, to: done.commit_id!, label: `上一轮（第 ${seq} 轮）的候选成果`, earlier: true };
  }
  return null;
}
