// Labels and pure transformations for display.

import type { BlobRef, Item, LiveItem, LiveTurn, Message, Phase, Turn } from './api/types';
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

export function sourceLabel(source: string): string {
  if (source === 'user') return '你';
  if (source === 'runtime') return 'Lobotomy';
  return source.replace(/^role:/, '');
}

export function turnOutcome(turn: Turn): string {
  if (turn.state === 'registered') return '等待启动';
  if (turn.state === 'running') return '运行中';
  if (turn.state === 'unknown') return '状态未知';
  if (turn.outcome === 'completed') return '已完成';
  if (turn.outcome === 'interrupted') return '被中断';
  return turn.failure?.kind === 'quota' ? '额度不足' : '失败';
}

export type Row =
  | { key: string; kind: 'turn'; turn: Turn }
  | { key: string; kind: 'message'; message: Message }
  | { key: string; kind: 'input'; turn: Turn }
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
        const messages = byTurn.get(turn.id) ?? [];
        for (const message of messages) rows.push({ key: `msg-${message.id}`, kind: 'message', message });
        // A continue note is not a message; the turn's input carries it.
        if (messages.length === 0 || turn.input.startsWith('【来自 Lobotomy】\n上一个 turn')) {
          rows.push({ key: `input-${turn.id}`, kind: 'input', turn });
        }
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
