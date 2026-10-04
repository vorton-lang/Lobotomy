// Ctrl+F over the thread (frontend.md §4.1 "搜索"). The browser's own find cannot see rows the
// virtualized list has not mounted, so the backend searches the whole thread and the thread
// scrolls to each match.

import { useEffect, useRef, useState } from 'react';
import { closeSearch, runSearch, stepSearch, useStore } from '../store';

const DEBOUNCE_MS = 200;

export function SearchBar() {
  const search = useStore((s) => s.search);
  const [text, setText] = useState(search?.query ?? '');
  const input = useRef<HTMLInputElement>(null);
  const focus = search?.focus;
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, [focus]);
  useEffect(() => {
    const timer = setTimeout(() => void runSearch(text), DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [text]);
  if (!search) return null;

  const count = search.matches.length;
  const label =
    !text.trim() ? '' : text !== search.query ? '…' : count === 0 ? '没有结果' : `${search.current + 1} / ${count}${search.more ? ' · 更早的未列出' : ''}`;
  return (
    <div className="search-bar" role="search">
      <input
        ref={input}
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="搜索对话"
        aria-label="搜索对话"
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            // The newest match comes first; Enter goes back in time, like the thread is read.
            void stepSearch(e.shiftKey ? 1 : -1);
          } else if (e.key === 'Escape') {
            closeSearch();
          }
        }}
      />
      <span className="muted search-count">{label}</span>
      <button className="ghost" onClick={() => void stepSearch(-1)} disabled={count === 0} aria-label="更早的匹配" title="更早（Enter）">
        ↑
      </button>
      <button className="ghost" onClick={() => void stepSearch(1)} disabled={count === 0} aria-label="更新的匹配" title="更新（Shift+Enter）">
        ↓
      </button>
      <button className="ghost" onClick={closeSearch} aria-label="关闭搜索">
        ✕
      </button>
    </div>
  );
}
