// The GUI's state: a projection of the backend (frontend.md §0). The backend is the authority;
// after any doubt (reconnect, resync) the store reloads instead of patching.

import { create } from 'zustand';
import { Connection, RequestFailed, type Status } from './api/connection';
import type { CommandRow, Item, LiveTurn, Message, Push, Snapshot, TaskDetail, ThreadPage, Turn } from './api/types';

export interface ThreadState {
  role: string;
  items: Item[];
  turns: Record<string, Turn>;
  messages: Message[];
  commands: Record<string, CommandRow>;
  queued: Message[];
  hasMore: boolean;
  loaded: boolean;
}

export interface Toast {
  id: number;
  text: string;
}

interface State {
  status: Status;
  snapshot: Snapshot | null;
  live: Record<string, LiveTurn>;
  thread: ThreadState;
  selectedTask: string | null;
  taskDetail: TaskDetail | null;
  toasts: Toast[];
}

/** M1 has one role in the main area (frontend.md §2 "M1 的布局"). */
export const MAIN_ROLE = 'Malkuth';

const emptyThread = (role: string): ThreadState => ({
  role,
  items: [],
  turns: {},
  messages: [],
  commands: {},
  queued: [],
  hasMore: false,
  loaded: false,
});

export const useStore = create<State>(() => ({
  status: 'connecting',
  snapshot: null,
  live: {},
  thread: emptyThread(MAIN_ROLE),
  selectedTask: null,
  taskDetail: null,
  toasts: [],
}));

let connection: Connection | null = null;

export function connect(url: string) {
  connection?.stop();
  connection = new Connection(url, {
    status: (status) => useStore.setState({ status }),
    opened: () => void reloadAll(),
    push: onPush,
  });
  connection.start();
}

export function call<T = unknown>(method: string, params: unknown = {}): Promise<T> {
  if (!connection) return Promise.reject(new Error('not connected'));
  return connection.call<T>(method, params);
}

let nextToast = 1;
export function toast(text: string) {
  const id = nextToast++;
  useStore.setState((s) => ({ toasts: [...s.toasts, { id, text }] }));
  setTimeout(() => useStore.setState((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })), 6_000);
}

/**
 * Runs a user command (data-model.md §9.2). The request id makes a retry harmless. Rejections
 * are shown to the user and returned as `undefined`.
 */
export async function run<T = unknown>(name: string, args: Record<string, unknown> = {}): Promise<T | undefined> {
  try {
    return await call<T>('command', { name, args: { request_id: crypto.randomUUID(), ...args } });
  } catch (e) {
    toast(e instanceof RequestFailed ? `${e.remote.message}` : String(e));
    return undefined;
  }
}

/** Runtime actions take no request id. */
export async function act<T = unknown>(name: string, args: Record<string, unknown> = {}): Promise<T | undefined> {
  try {
    return await call<T>('command', { name, args });
  } catch (e) {
    toast(e instanceof RequestFailed ? `${e.remote.message}` : String(e));
    return undefined;
  }
}

async function reloadAll() {
  await Promise.all([loadSnapshot(), loadNewestThread()]);
  const selected = useStore.getState().selectedTask;
  if (selected) await loadTaskDetail(selected);
}

async function loadSnapshot() {
  const snapshot = await call<Snapshot>('snapshot');
  useStore.setState({ snapshot, live: snapshot.live });
}

async function loadNewestThread() {
  const role = useStore.getState().thread.role;
  const page = await call<ThreadPage>('thread', { role });
  useStore.setState({ thread: { ...mergePage(emptyThread(role), page, 'replace'), loaded: true } });
}

export async function loadOlder() {
  const thread = useStore.getState().thread;
  if (!thread.hasMore || thread.items.length === 0) return;
  const page = await call<ThreadPage>('thread', { role: thread.role, before: thread.items[0].seq });
  useStore.setState((s) => ({ thread: mergePage(s.thread, page, 'older') }));
}

async function catchUp() {
  const thread = useStore.getState().thread;
  if (!thread.loaded) return;
  const after = thread.items.at(-1)?.seq ?? 0;
  const page = await call<ThreadPage>('thread', { role: thread.role, after });
  useStore.setState((s) => ({ thread: mergePage(s.thread, page, 'newer') }));
}

/** A turn can change state without new items (it ends); the newest page carries its state. */
async function refreshTurns() {
  const thread = useStore.getState().thread;
  if (!thread.loaded) return;
  const page = await call<ThreadPage>('thread', { role: thread.role });
  useStore.setState((s) => ({ thread: mergePage(s.thread, page, 'newer') }));
}

export function mergePage(thread: ThreadState, page: ThreadPage, mode: 'replace' | 'older' | 'newer'): ThreadState {
  const known = new Set(thread.items.map((i) => i.id));
  const fresh = page.items.filter((i) => !known.has(i.id));
  const items =
    mode === 'replace' ? page.items : mode === 'older' ? [...fresh, ...thread.items] : [...thread.items, ...fresh];
  const messages = new Map(thread.messages.map((m) => [m.id, m]));
  for (const m of page.messages) messages.set(m.id, m);
  return {
    ...thread,
    items,
    turns: { ...thread.turns, ...page.turns },
    messages: [...messages.values()],
    commands: { ...thread.commands, ...page.commands },
    queued: page.queued,
    hasMore: mode === 'newer' ? thread.hasMore : page.has_more,
  };
}

export async function loadTaskDetail(taskId: string) {
  const detail = await call<TaskDetail>('task', { task_id: taskId });
  if (useStore.getState().selectedTask === taskId) useStore.setState({ taskDetail: detail });
}

export function selectTask(taskId: string | null) {
  useStore.setState({ selectedTask: taskId, taskDetail: null });
  if (taskId) void loadTaskDetail(taskId);
}

/** Coalesces bursts of pushes into one reload per kind. */
function debounced(f: () => Promise<void>, ms: number) {
  let timer: ReturnType<typeof setTimeout> | null = null;
  return () => {
    if (timer) return;
    timer = setTimeout(() => {
      timer = null;
      f().catch(() => {});
    }, ms);
  };
}

const refreshSnapshot = debounced(loadSnapshot, 120);
const refreshThread = debounced(catchUp, 80);
const refreshTurnStates = debounced(refreshTurns, 120);
const refreshDetail = debounced(async () => {
  const selected = useStore.getState().selectedTask;
  if (selected) await loadTaskDetail(selected);
}, 150);

function onPush(push: Push) {
  switch (push.type) {
    case 'events': {
      refreshSnapshot();
      const selected = useStore.getState().selectedTask;
      if (selected && push.events.some((e) => e.entity === selected || e.payload?.task_id === selected)) refreshDetail();
      else if (selected && push.events.some((e) => e.kind.startsWith('verification.') || e.kind.startsWith('capture.'))) refreshDetail();
      if (push.events.some((e) => e.kind.startsWith('message.'))) refreshThread();
      if (push.events.some((e) => e.kind.startsWith('turn.'))) refreshTurnStates();
      break;
    }
    case 'thread':
      if (push.role === useStore.getState().thread.role) refreshThread();
      break;
    case 'live':
      useStore.setState({ live: push.live });
      break;
    case 'resync':
      void reloadAll();
      break;
    case 'tick':
      break;
  }
}
