// The GUI's state: a projection of the backend (frontend.md §0). The backend is the authority;
// after any doubt (reconnect, resync) the store reloads instead of patching.

import { useCallback, useRef, useState } from 'react';
import { create } from 'zustand';
import { Connection, RequestFailed, type Status } from './api/connection';
import { findBackend } from './bridge';
import type {
  CommandRecord,
  HostSnapshot,
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
  /** The projects and what the host shows across them (frontend.md §3 rule 8). */
  host: HostSnapshot | null;
  /**
   * The project the window shows; requests about a project name it. `null` is 「全部」: what waits
   * for the user in every project (frontend.md §2 "M3 的布局").
   */
  project: string | null;
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

/** What the window holds about the project it shows; another project starts from this. */
const emptyProject = () => ({
  snapshot: null,
  live: {},
  thread: emptyThread(''),
  following: true,
  selectedTask: null,
  taskDetail: null,
  search: null,
});

export const useStore = create<State>(() => ({
  status: 'connecting',
  host: null,
  project: null,
  ...emptyProject(),
  toasts: [],
}));

/** Whether the window still shows `project`: a reply about another one is dropped. */
const showing = (project: string | null) => useStore.getState().project === project;

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

/**
 * A request to the backend. Requests about the project name the one the window shows, unless they
 * name one themselves; `host` and the host's commands need none (frontend.md §3 rule 8).
 */
export function call<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
  if (!connection) return Promise.reject(new Error('not connected'));
  const project = useStore.getState().project;
  const scoped = method === 'host' || project === null || 'project' in params ? params : { ...params, project };
  return connection.call<T>(method, scoped);
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

/** The host snapshot says which project to show; its snapshot names the main role. */
async function reloadAll() {
  await loadHost();
  await reloadProject();
}

async function reloadProject() {
  if (!useStore.getState().project) return;
  await loadSnapshot();
  await loadNewestThread();
  const selected = useStore.getState().selectedTask;
  if (selected) await loadTaskDetail(selected);
}

/**
 * The project the window shows, or `null` for 「全部」. The window keeps its project while it stays
 * open; when it closes, the window shows 「全部」. At start, `initial`, the window shows the only
 * open project, or 「全部」 when there are several or none.
 */
export function chooseProject(host: HostSnapshot, current: string | null, initial: boolean): string | null {
  const open = host.projects.filter((p) => p.state === 'running' && p.error === null);
  if (current !== null && open.some((p) => p.id === current)) return current;
  return initial && open.length === 1 ? open[0].id : null;
}

/**
 * What waits for the user in all projects, for the window title and the tray: each project's,
 * and the host's blocked quota domains once (data-model.md §10.5).
 */
export function totalAttention(host: HostSnapshot | null): number {
  if (!host) return 0;
  return host.projects.reduce((n, p) => n + p.attention.length, host.attention.length);
}

let hostRequest = 0;
let appliedHost = 0;

/** Takes a new host snapshot. Returns whether the window now shows another project. */
async function loadHost(): Promise<boolean> {
  const request = ++hostRequest;
  const host = await call<HostSnapshot>('host');
  if (request < appliedHost) return false;
  appliedHost = request;
  const { project: current, host: before } = useStore.getState();
  const project = chooseProject(host, current, before === null);
  if (project === current) {
    useStore.setState({ host });
    return false;
  }
  useStore.setState({ host, project, ...emptyProject() });
  return true;
}

/**
 * Shows a project, or 「全部」 for `null`, and opens one of its tasks if `taskId` names one. The
 * project must be open: the list offers no others.
 */
export async function selectProject(project: string | null, taskId: string | null = null) {
  if (!showing(project)) {
    useStore.setState({ project, ...emptyProject() });
    try {
      await reloadProject();
    } catch (e) {
      toast(e instanceof RequestFailed ? e.remote.message : String(e));
      return;
    }
  }
  if (taskId && showing(project)) selectTask(taskId);
}

/** After the project list or a host setting changed. Host settings show in the project snapshot. */
export async function refreshHost() {
  if (await loadHost()) await reloadProject();
  else if (useStore.getState().project) await loadSnapshot();
}

// Refreshes can overlap: a host setting has no event sequence to order its snapshots.
let snapshotRequest = 0;
let appliedSnapshot = 0;

async function loadSnapshot() {
  const { project } = useStore.getState();
  if (project === null) return;
  const request = ++snapshotRequest;
  const snapshot = await call<Snapshot>('snapshot');
  if (request < appliedSnapshot || !showing(project)) return;
  appliedSnapshot = request;
  useStore.setState({ snapshot, live: snapshot.live });
}

/** The newest page replaces the thread; the list follows its end again. */
async function loadNewestThread() {
  const { project, snapshot } = useStore.getState();
  const role = mainRole(snapshot)?.name;
  if (!role) return;
  const page = await call<ThreadPage>('thread', { role });
  if (!showing(project)) return;
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
    const { project, thread } = useStore.getState();
    if (!thread.hasMore || thread.items.length === 0) return;
    const page = await call<ThreadPage>('thread', { role: thread.role, before: thread.items[0].seq });
    if (showing(project)) useStore.setState((s) => ({ thread: mergePage(s.thread, page, 'older') }));
  })().finally(() => (older = null));
  return older;
}

async function catchUp() {
  const { project, thread } = useStore.getState();
  if (!thread.loaded) return;
  const after = thread.items.at(-1)?.seq ?? 0;
  const page = await call<ThreadPage>('thread', { role: thread.role, after });
  if (showing(project)) useStore.setState((s) => ({ thread: mergePage(s.thread, page, 'newer') }));
}

/** A turn can change state without new items (it ends); the newest page carries its state. */
async function refreshTurns() {
  const { project, thread } = useStore.getState();
  if (!thread.loaded) return;
  const page = await call<ThreadPage>('thread', { role: thread.role });
  if (showing(project)) useStore.setState((s) => ({ thread: mergePage(s.thread, page, 'newer') }));
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
  const { project } = useStore.getState();
  const detail = await call<TaskDetail>('task', { task_id: taskId });
  if (showing(project) && useStore.getState().selectedTask === taskId) useStore.setState({ taskDetail: detail });
}

export function selectTask(taskId: string | null) {
  useStore.setState({ selectedTask: taskId, taskDetail: null });
  if (taskId) void loadTaskDetail(taskId);
}

// ---- search (frontend.md §4.1 "搜索") ----

export function openSearch() {
  // 「全部」 has no thread to search.
  if (useStore.getState().project === null) return;
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
const refreshHostSoon = debounced(refreshHost, 120);
/** The project list's counts and the window title follow every project's events. */
const refreshCounts = debounced(async () => {
  if (await loadHost()) await reloadProject();
}, 300);
const refreshThread = debounced(catchUp, 80);
const refreshTurnStates = debounced(refreshTurns, 120);
const refreshDetail = debounced(async () => {
  const selected = useStore.getState().selectedTask;
  if (selected) await loadTaskDetail(selected);
}, 150);

function onPush(push: Push) {
  if (push.type === 'events') refreshCounts();
  // Beyond the counts, the window shows only its own project.
  if ('project' in push && push.project !== useStore.getState().project) return;
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
    // A host setting or the project list changed; neither has an event of its own.
    case 'host':
      refreshHostSoon();
      break;
    case 'resync':
      void reloadAll();
      break;
    case 'tick':
      break;
  }
}
