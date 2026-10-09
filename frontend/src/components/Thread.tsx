// The main role's thread (frontend.md §2 "M1 的布局", §4.2): a virtualized list that loads older
// pages when scrolled to the top and follows the end while the user is at the end. Whether it
// follows is kept in the store (`following`). Away from the end, a button goes back to it and
// says when something new came (#18).

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { VList, type VListHandle } from 'virtua';
import type { CommandRecord } from '../api/types';
import { buildRows, clock, sourceLabel, turnOutcome, type Row } from '../format';
import { clearHighlight, highlight } from '../highlight';
import { loadOlder, matchRowKey, setFollowing, useStore } from '../store';
import { Markdown } from './common';
import { ItemView, LiveItemView, RunningView } from './ItemView';

const AT_END_SLACK = 48;

export function Thread() {
  const thread = useStore((s) => s.thread);
  const live = useStore((s) => s.live);
  const tasks = useStore((s) => s.snapshot?.tasks);
  const rows = useMemo(() => buildRows(thread, live), [thread, live]);
  const list = useRef<VListHandle>(null);
  const [prepending, setPrepending] = useState(false);
  const following = useStore((s) => s.following);

  // The list follows the end only while the user is there; scrolling is otherwise theirs. When
  // following starts again, such as from "回到最新", the list goes to the end.
  useLayoutEffect(() => {
    if (following && rows.length > 0) list.current?.scrollToIndex(rows.length - 1, { align: 'end' });
  }, [rows.length, following]);
  // The last row the user saw at the end. A different last row is new to them; older pages
  // loading above do not change it (#18).
  const lastKey = rows.at(-1)?.key;
  const seenLast = useRef(lastKey);
  useEffect(() => {
    if (following) seenLast.current = lastKey;
  }, [following, lastKey]);
  const fresh = !following && lastKey !== seenLast.current;
  // While an older page loads, the list keeps the visible rows in place as items arrive above
  // them (`shift`). Only the oldest item changing says the page is in: other changes, such as a
  // live item, may come first, and on a slow machine the page always comes later than the next
  // render. Ending the shift early left the list at its top with nothing more to load.
  const oldest = thread.items[0]?.id;
  useEffect(() => {
    setPrepending(false);
  }, [oldest]);

  // Search: scroll to the current match once its page is loaded, and mark the matches.
  const search = useStore((s) => s.search);
  const target = search && search.current >= 0 ? matchRowKey(search.matches[search.current]) : null;
  const handledJump = useRef(0);
  const toOpen = useRef<string | null>(null);
  useEffect(() => {
    if (!search || !target || handledJump.current === search.jump) return;
    const index = rows.findIndex((row) => row.key === target);
    // Not loaded yet: the search is loading older pages, and the rows change when they arrive.
    if (index < 0) return;
    handledJump.current = search.jump;
    setFollowing(false);
    toOpen.current = target;
    list.current?.scrollToIndex(index, { align: 'center' });
  }, [search, target, rows]);
  const query = search?.query ?? '';
  useEffect(() => {
    const root = document.querySelector('.thread');
    if (!root || !query.trim()) return;
    let frame = 0;
    const mark = () => {
      frame = 0;
      if (highlight(root, query.trim(), target, toOpen.current)) toOpen.current = null;
    };
    mark();
    // Rows mount and unmount as the list scrolls; their text arrives late (Markdown).
    const observer = new MutationObserver(() => {
      if (!frame) frame = requestAnimationFrame(mark);
    });
    observer.observe(root, { subtree: true, childList: true, characterData: true });
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
      clearHighlight();
    };
  }, [query, target, thread.loaded, rows.length === 0]);

  const onScroll = (offset: number) => {
    const handle = list.current;
    if (!handle) return;
    setFollowing(offset + handle.viewportSize >= handle.scrollSize - AT_END_SLACK);
    if (offset < 200 && thread.hasMore) {
      setPrepending(true);
      void loadOlder();
    }
  };

  if (!thread.loaded) return <div className="thread empty">正在加载对话…</div>;
  if (rows.length === 0) {
    return (
      <div className="thread empty">
        <p>还没有对话。在右侧建一个任务，或者直接给 {thread.role} 发消息。</p>
      </div>
    );
  }
  const title = (taskId: string | null) => (taskId ? tasks?.find((t) => t.id === taskId)?.title : undefined);
  return (
    <div className="thread-area">
      <VList ref={list} className="thread" shift={prepending} onScroll={onScroll} keepMounted={[rows.length - 1]}>
        {rows.map((row) => (
          <div key={row.key} className="row" data-key={row.key}>
            <RowView row={row} role={thread.role} title={title} commands={thread.commands} />
          </div>
        ))}
      </VList>
      {!following && (
        <button className={`to-latest ${fresh ? 'fresh' : ''}`} onClick={() => setFollowing(true)}>
          {fresh ? '有新内容 · 回到最新 ↓' : '回到最新 ↓'}
        </button>
      )}
    </div>
  );
}

function RowView({
  row,
  role,
  title,
  commands,
}: {
  row: Row;
  role: string;
  title: (taskId: string | null) => string | undefined;
  commands: Record<string, CommandRecord>;
}) {
  switch (row.kind) {
    case 'turn': {
      const t = row.turn;
      const name = title(t.task_id);
      return (
        <div className={`turn-divider ${t.outcome ?? t.state}`}>
          <span>{clock(t.started_at ?? t.registered_at)}</span>
          {name && <span>· {name}</span>}
          <span>· {turnOutcome(t)}</span>
          {t.failure && <span className="error">· {t.failure.message.split('\n')[0]}</span>}
        </div>
      );
    }
    case 'message':
      return <MessageView source={row.message.source} body={row.message.body} />;
    case 'input':
      return <MessageView source="runtime" body={row.note} />;
    case 'queued':
      return <MessageView source={row.message.source} body={row.message.body} queued />;
    case 'item':
      return <ItemView item={row.item} role={role} commands={commands} fold={row.fold} />;
    case 'live':
      return <LiveItemView item={row.item} />;
    case 'running':
      return <RunningView role={row.role} since={row.since} />;
  }
}

function MessageView({ source, body, queued }: { source: string; body: string; queued?: boolean }) {
  // The runtime's messages repeat the task and carry instructions for the executor; the first
  // line says what each is about, the rest opens on demand (#13).
  if (source === 'runtime') {
    const [summary, ...rest] = body.split('\n');
    const more = rest.join('\n').trim();
    return (
      <details className={`message runtime ${queued ? 'queued' : ''}`}>
        <summary>
          <span className="who">Lobotomy{queued && ' · 排队中'}</span> {summary}
        </summary>
        {more && <Markdown text={more} />}
      </details>
    );
  }
  return (
    <div className={`message ${source === 'user' ? 'mine' : 'other'} ${queued ? 'queued' : ''}`}>
      <div className="who">
        {sourceLabel(source)}
        {queued && <span className="muted"> · 排队中，下一个 turn 投递</span>}
      </div>
      <Markdown text={body} />
    </div>
  );
}
