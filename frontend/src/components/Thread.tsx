// Malkuth's thread (frontend.md §2 "M1 的布局", §4.2): a virtualized list that loads older pages
// when scrolled to the top and follows the end while the user is at the end.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { VList, type VListHandle } from 'virtua';
import { buildRows, clock, sourceLabel, turnOutcome, type Row } from '../format';
import { clearHighlight, highlight } from '../highlight';
import { loadOlder, matchRowKey, useStore } from '../store';
import { Markdown } from './common';
import { ItemView, LiveItemView, RunningView } from './ItemView';

const AT_END_SLACK = 48;

export function Thread() {
  const thread = useStore((s) => s.thread);
  const live = useStore((s) => s.live);
  const tasks = useStore((s) => s.snapshot?.tasks);
  const rows = useMemo(() => buildRows(thread, live), [thread, live]);
  const list = useRef<VListHandle>(null);
  const atEnd = useRef(true);
  const [prepending, setPrepending] = useState(false);
  const loading = useRef(false);

  // The list follows the end only while the user is there; scrolling is otherwise theirs.
  useLayoutEffect(() => {
    if (atEnd.current && rows.length > 0) list.current?.scrollToIndex(rows.length - 1, { align: 'end' });
  }, [rows.length]);
  useEffect(() => {
    if (prepending) setPrepending(false);
  }, [rows, prepending]);

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
    atEnd.current = false;
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
    atEnd.current = offset + handle.viewportSize >= handle.scrollSize - AT_END_SLACK;
    if (offset < 200 && thread.hasMore && !loading.current) {
      loading.current = true;
      setPrepending(true);
      void loadOlder().finally(() => (loading.current = false));
    }
  };

  if (!thread.loaded) return <div className="thread empty">正在加载对话…</div>;
  if (rows.length === 0) {
    return (
      <div className="thread empty">
        <p>还没有对话。在右侧建一个任务，或者直接给 Malkuth 发消息。</p>
      </div>
    );
  }
  const title = (taskId: string | null) => (taskId ? tasks?.find((t) => t.id === taskId)?.title : undefined);
  return (
    <VList ref={list} className="thread" shift={prepending} onScroll={onScroll} keepMounted={[rows.length - 1]}>
      {rows.map((row) => (
        <div key={row.key} className="row" data-key={row.key}>
          <RowView row={row} title={title} commands={thread.commands} />
        </div>
      ))}
    </VList>
  );
}

function RowView({
  row,
  title,
  commands,
}: {
  row: Row;
  title: (taskId: string | null) => string | undefined;
  commands: ReturnType<typeof useStore.getState>['thread']['commands'];
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
      return <ItemView item={row.item} commands={commands} />;
    case 'live':
      return <LiveItemView item={row.item} />;
    case 'running':
      return <RunningView since={row.since} />;
  }
}

function MessageView({ source, body, queued }: { source: string; body: string; queued?: boolean }) {
  const mine = source === 'user';
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
    <div className={`message ${mine ? 'mine' : source === 'runtime' ? 'runtime' : 'other'} ${queued ? 'queued' : ''}`}>
      <div className="who">
        {sourceLabel(source)}
        {queued && <span className="muted"> · 排队中，下一个 turn 投递</span>}
      </div>
      <Markdown text={body} />
    </div>
  );
}
