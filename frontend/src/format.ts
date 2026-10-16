// Labels and pure transformations for display. Components show what these return, so what the GUI
// says is tested without rendering (#16).

import type {
  Action,
  AttachmentView,
  Attempt,
  Attention,
  BlobRef,
  CommandRecord,
  Domain,
  Item,
  LedgerEntry,
  LedgerKind,
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
  Turn,
} from './api/types';
import type { ThreadState } from './store';

export const PHASE_LABEL: Record<Phase, string> = {
  queued: '排队',
  executing: '执行中',
  accepting: '待验收',
  done: '已完成',
  abandoned: '已放弃',
};

/**
 * The five looks of a state (ui-ux-plan.md §3 "状态"): each has its own shape, color and words,
 * so the state reads without color. Only `run` moves.
 */
export type Look = 'run' | 'wait' | 'fail' | 'done' | 'idle';

export const PHASE_LOOK: Record<Phase, Look> = {
  queued: 'idle',
  executing: 'run',
  accepting: 'wait',
  done: 'done',
  abandoned: 'idle',
};

/** The look of what `roleActivity` says, case by case. */
export function roleLook(role: RoleView, task: TaskView | undefined): Look {
  const turn = role.unfinished;
  if (turn) return turn.state === 'unknown' ? 'wait' : 'run';
  if (role.hold) return role.hold.kind === 'abnormal' ? 'fail' : 'wait';
  if (role.stalled) return 'wait';
  if (task?.phase === 'executing' && task.blocked_reason) return task.reply_queued ? 'idle' : 'wait';
  return task ? PHASE_LOOK[task.phase] : 'idle';
}

/** Roles with a color of their own; any other role gets one by its name. Six colors in all. */
const ROLE_HUE: Record<string, number> = { Angela: 0, Malkuth: 1, Binah: 2, Yesod: 3 };

export function roleHue(name: string): number {
  if (name in ROLE_HUE) return ROLE_HUE[name];
  let hash = 0;
  for (const c of name) hash = (hash * 31 + c.codePointAt(0)!) >>> 0;
  return hash % 6;
}

/**
 * How long a running turn has shown nothing new, said once it is two minutes or more. A fact, not
 * a verdict that the turn is stuck: a model may think that long (ui-ux-plan.md §8 第 1 条).
 */
export function quietFor(lastAt: number | undefined, now: number): string | null {
  if (lastAt === undefined) return null;
  const minutes = Math.floor((now - lastAt) / 60_000);
  return minutes >= 2 ? `${minutes} 分钟没有新输出` : null;
}

/** What changes as a live item grows: its kind, the size of its texts, and its status. */
export function liveSize(item: LiveItem): string {
  const content = jsonObject(item.content);
  let size = 0;
  for (const value of Object.values(content)) {
    if (typeof value === 'string') size += value.length;
    else if (isBlob(value)) size += value.size;
  }
  return `${item.kind}:${size}:${content.status ?? ''}`;
}

/** A command or tool of the running turn is still at work; it shows its own running time. */
export function toolRunning(turn: LiveTurn | undefined): boolean {
  return Object.values(turn?.items ?? {}).some((item) => jsonObject(item.content).status === 'in_progress');
}

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

/** Read only the fields a display needs from dynamic JSON; these are not wire validators. */
export function jsonObject(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

export function jsonArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

export function jsonNumber(value: unknown): number {
  return typeof value === 'number' ? value : 0;
}

export function isBlob(value: unknown): value is BlobRef {
  const ref = jsonObject(value);
  return (
    typeof ref.blob === 'string' &&
    typeof ref.size === 'number' &&
    typeof ref.head === 'string' &&
    typeof ref.tail === 'string'
  );
}

/** A text field as the thread shows it: whole, or the head and tail of one in the blob store. */
export function textOf(value: unknown): string {
  if (value == null) return '';
  return isBlob(value) ? `${value.head}\n…\n${value.tail}` : typeof value === 'string' ? value : '';
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
export function commandSummary(raw: unknown): { line: string; size: string | null } {
  const blob = isBlob(raw) ? raw : null;
  const command = blob ? blob.head : typeof raw === 'string' ? raw : '';
  const lines = command.split('\n');
  if (!blob && lines.length === 1 && command.length <= COMMAND_LINE) return { line: command, size: null };
  const line = lines[0].length > COMMAND_LINE ? `${lines[0].slice(0, COMMAND_LINE)}…` : lines[0];
  const size = blob ? `${blob.size} 个字符` : `共 ${lines.length} 行，${command.length} 个字符`;
  return { line, size };
}

/** The input fields that say what a tool works on, in the order they are looked for. */
const TOOL_SUBJECT = [
  'file_path',
  'notebook_path',
  'pattern',
  'path',
  'query',
  'url',
  'command',
  'subject',
  'description',
  'prompt',
];

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
  if (source === 'manager') return 'Angela';
  return source.replace(/^role:/, '');
}

export const ACTION_LABEL: Record<Action, string> = { read_only: '只读调查', edit_docs: '改文档', edit_code: '改代码' };

export const LEDGER_LABEL: Record<LedgerKind, string> = {
  goal: '目标',
  phase: '阶段',
  decision: '决定',
  constraint: '约束',
  open_question: '待定',
  hypothesis: '推测',
};

/** An entry still holds: no newer one superseded it, and its task's constraint has not lapsed. */
export const ledgerHolds = (e: LedgerEntry): boolean => e.superseded_by === null && !e.lapsed;

/**
 * The first line of a Markdown text that says something, as plain text cut to `max` characters:
 * what a reply shows of the message it replies to. Markup goes; a reference the runtime renders
 * shows as what it is. Lines of markup alone, such as a rule or a fence, are skipped.
 */
export function previewLine(text: string, max = 60): string {
  const plain = (line: string) =>
    line
      .replace(/^\s*(`{3,}|~{3,}).*$/, '')
      .replace(REFERENCE, (_, kind: string) => (kind === 'quote' ? '（原话）' : '（附件）'))
      .replace(/^\s*(?:(?:#{1,6}|>|[-*+]|\d+[.)])\s+|>)+/, '')
      .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
      .replace(/(\*\*|__|~~)(.+?)\1/g, '$2')
      .replace(/\*([^*\s][^*]*)\*/g, '$1')
      .replace(/`([^`]*)`/g, '$1')
      .trim();
  const line =
    text
      .split('\n')
      .map(plain)
      .find((l) => /[\p{L}\p{N}]/u.test(l)) ?? '';
  return line.length > max ? `${line.slice(0, max)}…` : line;
}

/** A piece of Angela's reply: her own text, or a reference the runtime renders (manager-actions.md §2.1). */
export type ReplyPart = { kind: 'text'; text: string } | { kind: 'quote' | 'att'; id: string };

const REFERENCE = /\[\[(quote|att):([A-Za-z0-9_]+)\]\]/g;

/** Splits Angela's reply at its `[[quote:…]]` and `[[att:…]]` references. */
export function splitReply(text: string): ReplyPart[] {
  const parts: ReplyPart[] = [];
  let last = 0;
  for (const match of text.matchAll(REFERENCE)) {
    const before = text.slice(last, match.index);
    if (before.trim()) parts.push({ kind: 'text', text: before });
    parts.push({ kind: match[1] as 'quote' | 'att', id: match[2] });
    last = match.index + match[0].length;
  }
  const rest = text.slice(last);
  if (rest.trim() || parts.length === 0) parts.push({ kind: 'text', text: rest });
  return parts;
}

/**
 * An attachment's line: its project, what it is and its source. The
 * runtime's record says all of it; Angela cannot write it.
 */
export function attachmentLine(a: AttachmentView): string {
  const where = `${a.project_name} · `;
  switch (a.kind) {
    case 'task':
      return `${where}任务「${a.title}」 · ${a.executor} · ${PHASE_LABEL[a.phase]}${a.paused ? ' · 已暂停' : ''}`;
    case 'report':
      return `${where}${a.role} 的汇报 · ${a.report_title} · ${clock(a.at)} · ${a.chars} 字`;
  }
}

/** Why an attachment no longer holds now, or `null` while it does. */
export function attachmentExpired(a: AttachmentView): string | null {
  if (a.kind === 'report' && a.effect && VOID_REPORT[a.effect]) return '没有生效';
  return null;
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
  if (role.stalled) return 'turn 已结束，任务还没完成，等你发消息';
  // The question stands until a reply from the user or Angela is bound to the next turn (#17).
  if (task?.phase === 'executing' && task.blocked_reason) {
    return task.reply_queued ? '回复在排队，下一个 turn 交给它' : '提出了问题，等你回答';
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
  return `任务「${task.title}」等你验收。消息先排队，退回后交给 ${role}；直接验收则不再投递`;
}

/** Where the task stands, in one line: which round, and what it waits for. */
export function taskNow(phase: Phase, round: number | undefined): string {
  switch (phase) {
    case 'queued':
      return round ? `排队中，轮到时开始第 ${round + 1} 轮执行。` : '排队中，还没有开始执行。';
    case 'executing':
      return `第 ${round} 轮执行中，还没有交出候选成果。`;
    case 'accepting':
      return `第 ${round} 轮交出了候选成果，等你验收。`;
    case 'done':
      return `已完成，验收的是第 ${round} 轮的候选成果。`;
    case 'abandoned':
      return '已放弃。';
  }
}

/** A blocked quota domain, in one line (data-model.md §8.7). */
export function quotaNote(domain: Domain): string {
  const reset = domain.resets_at ? ` · ${clock(domain.resets_at)} 重置` : ' · 重置时间未知';
  return `额度不足（${harnessName(domain.harness)}）${reset} · 等待手动重试`;
}

export const REPORT_LABEL: Record<ReportArgs['status'], string> = {
  progress: '进展',
  blocked: '需要你决定',
  done: '完成',
};

/** Why a report the backend recorded changed nothing (#14). */
const VOID_REPORT: Partial<Record<ReportEffect, string>> = {
  late: '这次报告没有生效：它所属的执行轮已经结束，报告没有推进任务。',
  no_task: '这次报告没有生效：这个 turn 不属于任何任务，报告没有推进任务。',
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
 * goes to the project to act on one (frontend.md §2).
 */
export function attentionSummary(a: Attention): string {
  switch (a.kind) {
    case 'hold':
      if (a.hold.kind === 'abnormal') return `${a.role} 的上一个 turn ${abnormalEnd(a.hold.turn)}，停下等你决定`;
      return `${a.role} 的会话没有记下会话 ID，要开新会话`;
    case 'unknown_turn':
      return `后端重启前启动的 ${a.role} 的 CLI 仍在运行`;
    case 'task_blocked':
      return `「${a.title}」的执行者在等你回答`;
    case 'stalled':
      return `「${a.title}」没有完成，${a.role} 的 turn 已经结束，等你发消息`;
    case 'accept':
      return `「${a.title}」交出了候选成果，等你验收`;
    case 'quota':
      return quotaNote(a.domain);
    case 'question':
      return `Angela 问你：${a.question}`;
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
    case 'unknown_turn':
      return `unknown:${a.turn_id}`;
    case 'task_blocked':
      return `blocked:${a.task_id}`;
    case 'stalled':
      return `stalled:${a.turn_id}`;
    case 'accept':
      return `accept:${a.task_id}`;
    case 'quota':
      return `quota:${a.domain.harness}`;
    case 'question':
      return `question:${a.id}`;
  }
}

/**
 * The note a continue put in front of the turn's input, without what follows it: the queued
 * messages, which show as messages of their own, and the runtime's other blocks, such as the
 * user's local time in Angela's input. The format is the backend's (`register` in turn.rs).
 */
export function continueNote(input: string): string | null {
  const head = '【来自 Lobotomy】\n';
  if (!input.startsWith(head)) return null;
  const rest = input.slice(head.length);
  const end = rest.indexOf('\n\n【');
  return end < 0 ? rest : rest.slice(0, end);
}

export type Row =
  | { key: string; kind: 'turn'; turn: Turn }
  | { key: string; kind: 'message'; message: Message }
  | { key: string; kind: 'input'; turn: Turn; note: string }
  /** `fold`: a done report that the turn's last message sums up again; its body starts folded (#18). */
  | { key: string; kind: 'item'; item: Item; fold: boolean }
  /** `last`: the running turn's newest item, under which a quiet turn says so. */
  | { key: string; kind: 'live'; turnId: string; itemId: string; item: LiveItem; last: boolean }
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
        for (const message of byTurn.get(turn.id) ?? [])
          rows.push({ key: `msg-${message.id}`, kind: 'message', message });
      }
    }
    if (item.kind === 'input') continue;
    rows.push({ key: `item-${item.id}`, kind: 'item', item, fold: folded.has(item.id) });
  }
  for (const [turnId, turn] of Object.entries(live)) {
    if (turn.role !== thread.role) continue;
    const items = Object.entries(turn.items);
    if (items.length === 0)
      rows.push({ key: `running-${turnId}`, kind: 'running', turnId, role: turn.role, since: turn.started_at });
    items.forEach(([itemId, item], i) =>
      rows.push({ key: `live-${turnId}-${itemId}`, kind: 'live', turnId, itemId, item, last: i === items.length - 1 }),
    );
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
};

/** A decision in the task's history, said of who made it: the user or Angela. */
function decisionLabel(kind: string, actor: string): string {
  const label = DECISION_LABEL[kind] ?? kind;
  return actor === 'manager' ? `Angela ${label.replace(/^你/, '')}` : label;
}

/** What a closing decision let go: the user's messages the executor never got (#14). */
function droppedNote(detail: unknown): string {
  const dropped = jsonArray(jsonObject(detail).dropped_messages)
    .map(jsonObject)
    .map((message) => ({ body: textOf(message.body) }));
  if (dropped.length === 0) return '';
  const quoted = dropped.map((m) => `「${m.body.length > 40 ? `${m.body.slice(0, 40)}…` : m.body}」`).join('');
  return `；${dropped.length} 条消息没有投递：${quoted}`;
}

export interface TimelineEntry {
  at: number;
  text: string;
}

/**
 * A task's history in time order: execution rounds, decisions and criteria changes (#13). What one command did shares a timestamp; the rank puts its cause first,
 * e.g. a send-back before the round it opens.
 */
export function timeline(detail: TaskDetail): TimelineEntry[] {
  const entries: (TimelineEntry & { rank: number })[] = [];
  const add = (at: number | null, rank: number, text: string) => at && entries.push({ at, rank, text });
  add(detail.task.created_at, 0, detail.task.assignment ? 'Angela 派出任务' : '建立任务');
  for (const c of detail.criteria) if (c.version > 1) add(c.created_at, 0, `完成条件改为第 ${c.version} 版`);
  for (const d of detail.decisions) {
    const body = textOf(jsonObject(d.detail).reason);
    const reason = body ? `：${body}` : '';
    add(d.created_at, 0, `${decisionLabel(d.kind, d.actor)}${reason}${droppedNote(d.detail)}`);
  }
  for (const a of detail.attempts) {
    add(a.started_at, 2, `第 ${a.seq} 轮执行开始`);
    add(a.ended_at, 1, a.end_reason === 'candidate' ? `第 ${a.seq} 轮交出候选成果` : `第 ${a.seq} 轮结束`);
  }
  return entries.sort((a, b) => a.at - b.at || a.rank - b.rank).map(({ at, text }) => ({ at, text }));
}

// A done report arrives before its turn ends and the attempt hands in its candidate. Keep that
// distinction visible, and never present a previous round's report as the current delivery.
export function deliveryReport(attempts: Attempt[]) {
  const report = attempts.findLast((attempt) => !!attempt.done_summary?.trim());
  if (!report) return null;
  return {
    text: report.done_summary!,
    round: report.seq,
    earlier: report.id !== attempts.at(-1)?.id,
    pending: report.end_reason !== 'candidate',
  };
}
