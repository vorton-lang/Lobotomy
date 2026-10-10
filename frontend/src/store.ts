// The GUI's state: a projection of the backend (frontend.md §0). The backend is the authority;
// after any doubt (reconnect, resync) the store reloads instead of patching.

import { useCallback, useRef, useState } from 'react';
import { create } from 'zustand';
import { Connection, RequestFailed, type Status } from './api/connection';
import { findBackend } from './bridge';
import type {
  CommandRecord,
  Item,
  LiveTurn,
  Message,
  Permission,
  Push,
  RoleView,
  SearchMatch,
  Snapshot,
  TaskDetail,
  ThreadPage,
  Turn,
} from './api/types';

export interface ThreadState {
  role: string;
  items: Item[];
  turns: Record<string, Turn>;
  messages: Message[];
  commands: Record<string, CommandRecord>;
  queued: Message[];
  hasMore: boolean;
  loaded: boolean;
}

export interface Toast {
  id: number;
  text: string;
}

/** Ctrl+F in the thread (frontend.md §4.1 "搜索"). */
export interface SearchState {
  query: string;
  matches: SearchMatch[];
  /** Older matches were left out. */
  more: boolean;
  /** Index into `matches`; the newest at first. */
  current: number;
  /** Bumped by every jump, so the thread scrolls even to the same match again. */
  jump: number;
  /** Bumped by every Ctrl+F, so the search field takes the focus again. */
  focus: number;
}

interface State {
  status: Status;
  snapshot: Snapshot | null;
  live: Record<string, LiveTurn>;
  thread: ThreadState;
  /**
   * The thread follows its end: new rows scroll into view. True while the user is at the end;
   * scrolling up, or going to a search match, stops it.
   */
  following: boolean;
  selectedTask: string | null;
  taskDetail: TaskDetail | null;
  toasts: Toast[];
  search: SearchState | null;
}

/** The role whose thread fills the main area. M1 has one, its executor (frontend.md §2 "M1 的布局"). */
export function mainRole(snapshot: Snapshot | null): RoleView | undefined {
  return snapshot?.roles.find((r) => r.kind === 'worker') ?? snapshot?.roles[0];
}

export const useMainRole = () => useStore((s) => mainRole(s.snapshot));

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
  thread: emptyThread(''),
  following: true,
  selectedTask: null,
  taskDetail: null,
  toasts: [],
  search: null,
}));

let connection: Connection | null = null;

export function connect(url: string) {
  connection?.stop();
  connection = new Connection(
    url,
    {
      status: (status) => useStore.setState({ status }),
      opened: () => void reloadAll(),
      push: onPush,
    },
    findBackend,
  );
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
export async function run<T = unknown>(
  name: string,
  args: Record<string, unknown> = {},
  requestId: string = crypto.randomUUID(),
): Promise<T | undefined> {
  return act<T>(name, { request_id: requestId, ...args });
}

/**
 * A command that creates something or sends text, submitted from a form (#16). While a request
 * is out, `busy` is true and another submit does nothing. The request id stays the same until a
 * submit succeeds, so pressing again after a reply was lost runs the command once.
 */
export function useCommand<T = unknown>(name: string) {
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const requestId = useRef(crypto.randomUUID());
  const submit = useCallback(
    async (args: Record<string, unknown>): Promise<T | undefined> => {
      if (pending.current) return undefined;
      pending.current = true;
      setBusy(true);
      try {
        const result = await run<T>(name, args, requestId.current);
        if (result !== undefined) requestId.current = crypto.randomUUID();
        return result;
      } finally {
        pending.current = false;
        setBusy(false);
      }
    },
    [name],
  );
  return { submit, busy };
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

/**
 * Changes one harness's permission mode, a host setting (harness-adapter.md §1.9). Every window
 * takes a new snapshot when the backend says so.
 */
export function setPermission(harness: string, permission: Permission) {
  return act('set_permission', { harness, permission });
}

/** The snapshot names the main role, whose thread loads next. */
async function reloadAll() {
  await loadSnapshot();
  await loadNewestThread();
  const selected = useStore.getState().selectedTask;
  if (selected) await loadTaskDetail(selected);
}

// Refreshes can overlap: a host setting has no event sequence to order its snapshots.
let snapshotRequest = 0;
let appliedSnapshot = 0;

async function loadSnapshot() {
  const request = ++snapshotRequest;
  const snapshot = await call<Snapshot>('snapshot');
  if (request < appliedSnapshot) return;
  appliedSnapshot = request;
  useStore.setState({ snapshot, live: snapshot.live });
}

/** The newest page replaces the thread; the list follows its end again. */
async function loadNewestThread() {
  const role = mainRole(useStore.getState().snapshot)?.name;
  if (!role) return;
  const page = await call<ThreadPage>('thread', { role });
  useStore.setState({ thread: { ...mergePage(emptyThread(role), page, 'replace'), loaded: true }, following: true });
}

/** Called on every scroll; changes the state only when following starts or stops. */
export function setFollowing(following: boolean) {
  if (useStore.getState().following !== following) useStore.setState({ following });
}

let older: Promise<void> | null = null;

/** Loads the page before the oldest item. A call while a page loads waits for that one. */
export function loadOlder(): Promise<void> {
  older ??= (async () => {
    const thread = useStore.getState().thread;
    if (!thread.hasMore || thread.items.length === 0) return;
    const page = await call<ThreadPage>('thread', { role: thread.role, before: thread.items[0].seq });
    useStore.setState((s) => ({ thread: mergePage(s.thread, page, 'older') }));
  })().finally(() => (older = null));
  return older;
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

// ---- search (frontend.md §4.1 "搜索") ----

export function openSearch() {
  const search = useStore.getState().search;
  useStore.setState({ search: search ? { ...search, focus: search.focus + 1 } : { query: '', matches: [], more: false, current: -1, jump: 0, focus: 1 } });
  // A task panel would cover the matches.
  selectTask(null);
}

export function closeSearch() {
  useStore.setState({ search: null });
}

let searchRequest = 0;

/** Searches the whole thread on the backend and goes to the newest match. */
export async function runSearch(query: string) {
  const request = ++searchRequest;
  const role = useStore.getState().thread.role;
  const found = query.trim()
    ? await call<{ matches: SearchMatch[]; more: boolean }>('search', { role, query }).catch((e) => {
        toast(String(e));
        return { matches: [], more: false };
      })
    : { matches: [], more: false };
  if (request !== searchRequest || !useStore.getState().search) return;
  const current = found.matches.length - 1;
  useStore.setState((s) => ({ search: s.search && { ...s.search, query, ...found, current, jump: s.search.jump + 1 } }));
  if (current >= 0) await reveal(found.matches[current]);
}

/** Goes to an older (-1) or newer (+1) match, round the ends. */
export async function stepSearch(delta: number) {
  const search = useStore.getState().search;
  if (!search || search.matches.length === 0) return;
  const current = (search.current + delta + search.matches.length) % search.matches.length;
  useStore.setState({ search: { ...search, current, jump: search.jump + 1 } });
  await reveal(search.matches[current]);
}

/** Loads older pages until the match's row is in the thread. */
async function reveal(match: SearchMatch) {
  if (match.seq === null) return;
  try {
    for (;;) {
      const thread = useStore.getState().thread;
      const oldest = thread.items[0]?.seq;
      if (oldest === undefined || oldest <= match.seq || !thread.hasMore) return;
      await loadOlder();
    }
  } catch (e) {
    toast(String(e));
  }
}

/** The thread row a match is shown in (`buildRows`). */
export function matchRowKey(match: SearchMatch): string {
  if (match.kind === 'item') return `item-${match.id}`;
  return match.seq === null ? `queued-${match.id}` : `msg-${match.id}`;
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
    // A host setting changed; it has no event of its own.
    case 'host':
      refreshSnapshot();
      break;
    case 'resync':
      void reloadAll();
      break;
    case 'tick':
      break;
  }
}
