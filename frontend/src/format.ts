// Labels and pure transformations for display. Components show what these return, so what the GUI
// says is tested without rendering (#16).

import type {
  Attempt,
  Attention,
  BlobRef,
  Capture,
  CheckRun,
  CommandRecord,
  Domain,
  Item,
  LiveItem,
  LiveTurn,
  Message,
  Permission,
  Phase,
  ReportArgs,
  ReportEffect,
  ReportRecord,
  RoleView,
  TaskDetail,
  TaskView,
  Text,
  Turn,
  Verification,
} from './api/types';
import type { ThreadState } from './store';

export const PHASE_LABEL: Record<Phase, string> = {
  queued: '排队',
  executing: '执行中',
  verifying: '验证中',
  accepting: '待验收',
  done: '已完成',
  abandoned: '已放弃',
};

export const VERIFICATION_LABEL: Record<Verification['state'], string> = { running: '进行中', passed: '通过', failed: '未通过' };

export const PERMISSION_LABEL: Record<Permission, string> = { full: '完全放开', auto_review: '自动审批' };

const HARNESS_LABEL: Record<string, string> = { codex: 'Codex', claude: 'Claude' };

/** A harness's product name, such as Codex for `codex`. */
export function harnessName(harness: string): string {
  return HARNESS_LABEL[harness] ?? harness;
}

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

/** A text field as the thread shows it: whole, or the head and tail of one in the blob store. */
export function textOf(value: Text | null | undefined): string {
  if (value == null) return '';
  return isBlob(value) ? `${value.head}\n…\n${value.tail}` : value;
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

/** A command's first line shows at most this many characters while folded. */
const COMMAND_LINE = 120;

/**
 * How a command shows folded (#14): a short one whole; a long one, such as a heredoc that writes a
 * file, by its first line, with its size.
 */
export function commandSummary(raw: Text | null | undefined): { line: string; size: string | null } {
  const command = isBlob(raw) ? raw.head : (raw ?? '');
  const lines = command.split('\n');
  if (!isBlob(raw) && lines.length === 1 && command.length <= COMMAND_LINE) return { line: command, size: null };
  const line = lines[0].length > COMMAND_LINE ? `${lines[0].slice(0, COMMAND_LINE)}…` : lines[0];
  const size = isBlob(raw) ? `${raw.size} 个字符` : `共 ${lines.length} 行，${command.length} 个字符`;
  return { line, size };
}

/** The input fields that say what a tool works on, in the order they are looked for. */
const TOOL_SUBJECT = ['file_path', 'notebook_path', 'pattern', 'path', 'query', 'url', 'command', 'subject', 'description', 'prompt'];

/**
 * What a tool call of the harness's own works on, in one line: the file it reads, the pattern it
 * searches, the task it creates. Empty when its input names none of these.
 */
export function toolSubject(input: unknown): string {
  if (typeof input !== 'object' || input === null) return '';
  const fields = input as Record<string, unknown>;
  const value = TOOL_SUBJECT.map((key) => fields[key]).find((v) => typeof v === 'string' && v.trim());
  if (typeof value !== 'string') return '';
  const line = value.split('\n')[0];
  return line.length > COMMAND_LINE ? `${line.slice(0, COMMAND_LINE)}…` : line;
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

/** What the role does now, in a few words, for its line in the Workboard. */
export function roleActivity(role: RoleView, task: TaskView | undefined, now: number): string {
  const turn = role.unfinished;
  if (turn?.state === 'running' && turn.started_at) return `正在跑 turn · ${elapsed(turn.started_at, now)}`;
  if (turn) return turn.state === 'unknown' ? '上一个 turn 状态未知' : '即将开始 turn';
  if (role.hold) return '停下，等你决定';
  if (role.outside) return '任务之外有改动，等你决定';
  if (role.stalled) return 'turn 已结束，任务还没完成，等你发消息';
  if (role.workspace?.state === 'materializing') return '正在准备工作目录';
  // The question stands until the user's reply is bound to the next turn (#17).
  if (task?.phase === 'executing' && task.blocked_reason) {
    return task.undelivered_messages > 0 ? '你的回复在排队，下一个 turn 交给它' : '提出了问题，等你回答';
  }
  if (task) return PHASE_LABEL[task.phase];
  return '空闲';
}

/**
 * Where a message to the role goes, said before it is sent. While the role works on a task, the
 * message belongs to it; outside execution it waits for the task to return to execution, and
 * accepting the task lets it go (data-model.md §4.2, #14, #15).
 */
export function composerHint(role: string, task: TaskView | undefined): string {
  if (!task) return `发给 ${role}（不属于任何任务）`;
  if (task.phase === 'executing') return `发给 ${role} · 任务「${task.title}」`;
  if (task.phase === 'verifying') {
    return `任务「${task.title}」正在验证。消息先排队，任务回到执行时交给 ${role}；验收前要你决定退回还是不再投递`;
  }
  return `任务「${task.title}」等你验收。消息先排队，退回后交给 ${role}；直接验收则不再投递`;
}

/** Where the task stands, in one line: which round, and what it waits for. */
export function taskNow(phase: Phase, round: number | undefined, verification: Verification['state'] | null): string {
  switch (phase) {
    case 'queued':
      return round ? `排队中，轮到时开始第 ${round + 1} 轮执行。` : '排队中，还没有开始执行。';
    case 'executing':
      return `第 ${round} 轮执行中，还没有交出候选成果。`;
    case 'verifying':
      return verification === 'running' ? `第 ${round} 轮的候选成果正在验证。` : `第 ${round} 轮的候选成果等待验证。`;
    case 'accepting':
      return `第 ${round} 轮的候选成果通过了验证，等你验收。`;
    case 'done':
      return `已完成，验收的是第 ${round} 轮的候选成果。`;
    case 'abandoned':
      return '已放弃。';
  }
}

export function checkPassed(check: CheckRun): boolean {
  return !check.timed_out && check.exit_code === 0;
}

/** A check's result in one line: its exit code or the timeout, and how long it ran. */
export function checkSummary(check: CheckRun): string {
  return `${check.timed_out ? '超时' : `退出码 ${check.exit_code}`} · ${(check.duration_ms / 1000).toFixed(1)} 秒`;
}

/** A blocked quota domain, in one line (data-model.md §8.7). */
export function quotaNote(domain: Domain): string {
  const reset = domain.resets_at ? ` · ${clock(domain.resets_at)} 重置` : ' · 重置时间未知';
  return `额度不足（${harnessName(domain.harness)}）${reset} · 等待手动重试`;
}

/** What a capture the runtime stopped holds back, one line per path. */
export function stoppedPaths(capture: Capture): string[] {
  if (capture.state === 'oversized') return (capture.detail?.files ?? []).map((f) => `${f.path}（${bytes(f.size)}）`);
  return (capture.detail?.paths ?? []).map((p) => `${p.path}（${p.kind}）`);
}

export const REPORT_LABEL: Record<ReportArgs['status'], string> = { progress: '进展', blocked: '需要你决定', done: '完成' };

/** Why a report the backend recorded changed nothing (#14). */
const VOID_REPORT: Partial<Record<ReportEffect, string>> = {
  late: '这次报告没有生效：它所属的执行轮已经结束，报告没有推进任务。',
  no_task: '这次报告没有生效：这个 turn 不属于任何任务，报告没有推进任务。它做的改动不会被验收或发布，要保留就建成任务。',
};

/** The report a Lobotomy tool call recorded, if it was one. */
export function asReport(record: CommandRecord | undefined): ReportRecord | null {
  return record?.name === 'org_report' ? (record as unknown as ReportRecord) : null;
}

/** Why the report changed nothing, or `null` when it took effect: what it did, not what it asked for (#14). */
export function voidReport(report: ReportRecord): string | null {
  return VOID_REPORT[report.result] ?? null;
}

/** A stable key for a card that waits for the user: a card keeps its draft while others come and go (#16). */
/**
 * One line for an item of "等你决定" in 「全部」, where the user sees every project's items and
 * goes to the project to act on one (frontend.md §2 "M3 的布局").
 */
export function attentionSummary(a: Attention): string {
  switch (a.kind) {
    case 'hold':
      if (a.hold.kind === 'abnormal') return `${a.role} 的上一个 turn ${abnormalEnd(a.hold.turn)}，停下等你决定`;
      if (a.hold.kind === 'session_unidentified') return `${a.role} 的会话没有记下会话 ID，要开新会话`;
      return a.hold.capture.state === 'oversized'
        ? `${a.role} 的这次采集超过了体积上限，等你决定`
        : `${a.role} 的工作目录里有采集无法保存的内容，等你决定`;
    case 'outside_changes':
      return `${a.role} 在任务之外改了 ${a.capture.detail?.changed?.length ?? 0} 个文件，等你决定`;
    case 'unknown_turn':
      return `后端重启前启动的 ${a.role} 的 CLI 仍在运行`;
    case 'task_blocked':
      return `「${a.title}」的执行者在等你回答`;
    case 'stalled':
      return `「${a.title}」没有完成，${a.role} 的 turn 已经结束，等你发消息`;
    case 'accept':
      return `「${a.title}」通过了验证，等你验收`;
    case 'quota':
      return quotaNote(a.domain);
    case 'job_failed':
      return `运行时的工作出错：${a.key}`;
    case 'preview_stopped':
      return `预览已停止：${a.reason}`;
  }
}

/** The task an item of "等你决定" is about, which going to its project opens. */
export function attentionTask(a: Attention): string | null {
  return a.kind === 'accept' || a.kind === 'task_blocked' || a.kind === 'stalled' ? a.task_id : null;
}

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
  /** `fold`: a done report that the turn's last message sums up again; its body starts folded (#18). */
  | { key: string; kind: 'item'; item: Item; fold: boolean }
  | { key: string; kind: 'live'; turnId: string; itemId: string; item: LiveItem }
  | { key: string; kind: 'running'; turnId: string; role: string; since: number }
  | { key: string; kind: 'queued'; message: Message };

/**
 * The thread as rows: each turn opens with a divider and the messages it delivered, then its
 * items. Items of turns still running follow, then messages waiting for a turn.
 */
export function buildRows(thread: ThreadState, live: Record<string, LiveTurn>): Row[] {
  const rows: Row[] = [];
  const folded = repeatedReports(thread);
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
    rows.push({ key: `item-${item.id}`, kind: 'item', item, fold: folded.has(item.id) });
  }
  for (const [turnId, turn] of Object.entries(live)) {
    if (turn.role !== thread.role) continue;
    const items = Object.entries(turn.items);
    if (items.length === 0) rows.push({ key: `running-${turnId}`, kind: 'running', turnId, role: turn.role, since: turn.started_at });
    for (const [itemId, item] of items) rows.push({ key: `live-${turnId}-${itemId}`, kind: 'live', turnId, itemId, item });
  }
  for (const message of thread.queued) rows.push({ key: `queued-${message.id}`, kind: 'queued', message });
  return rows;
}

/**
 * The done reports that a later message of the same turn follows. The executor's last message
 * usually sums up the work again, so the report shows its status and title, and its body starts
 * folded (#18).
 */
function repeatedReports(thread: ThreadState): Set<string> {
  const lastMessage = new Map<string, number>();
  for (const item of thread.items) if (item.kind === 'agent_message') lastMessage.set(item.turn_id, item.seq);
  const out = new Set<string>();
  for (const item of thread.items) {
    if (item.kind !== 'mcp_call' || !item.command_id) continue;
    const done = asReport(thread.commands[item.command_id])?.args.status === 'done';
    if (done && (lastMessage.get(item.turn_id) ?? -Infinity) > item.seq) out.add(item.id);
  }
  return out;
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
      const result = v.state === 'passed' ? '通过' : v.conflicts.length ? '未通过：有冲突' : '未通过';
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

// A done report arrives before candidate capture. Keep that distinction visible, and never
// present a previous round's report as the current delivery. Invalidated reports are cleared.
export function deliveryReport(attempts: Attempt[]) {
  const report = attempts.findLast((attempt) => !!attempt.done_summary?.trim());
  if (!report) return null;
  return {
    text: report.done_summary!,
    round: report.seq,
    earlier: report.id !== attempts.at(-1)?.id,
    pending: !report.candidate_id,
  };
}

/** Only the current round's explicitly pinned candidate can be launched. */
export function trialCandidate(detail: TaskDetail): string | null {
  const latest = detail.attempts.at(-1);
  if (!latest?.candidate_id) return null;
  return detail.captures.find((capture) =>
    capture.id === latest.candidate_id && capture.task_id === detail.task.id &&
    capture.attempt_id === latest.id && capture.kind === 'candidate' && capture.state === 'pinned',
  )?.commit_id ?? null;
}
