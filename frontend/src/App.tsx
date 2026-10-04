// The M1 window (frontend.md §2 "M1 的布局"): Malkuth's thread in the main area, what waits for
// the user and the Workboard on the right, a task's detail in a panel over the thread.

import { useEffect, useState } from 'react';
import { backendUrl, inElectron } from './bridge';
import { Composer } from './components/Composer';
import { Onboarding } from './components/Onboarding';
import { SearchBar } from './components/SearchBar';
import { Sidebar } from './components/Sidebar';
import { TaskPanel } from './components/TaskPanel';
import { Thread } from './components/Thread';
import { TopBar } from './components/TopBar';
import { connect, openSearch, useStore } from './store';

export function App() {
  const [url, setUrl] = useState<string | null | undefined>(undefined);
  // Why the backend could not be reached at start, such as a backend that did not start (#16).
  const [failed, setFailed] = useState<string | null>(null);
  const snapshot = useStore((s) => s.snapshot);
  const selectedTask = useStore((s) => s.selectedTask);
  const toasts = useStore((s) => s.toasts);
  const searching = useStore((s) => s.search !== null);
  const attention = snapshot?.attention.length ?? 0;

  const find = () => {
    setFailed(null);
    backendUrl().then(setUrl, (e: unknown) => setFailed(e instanceof Error ? e.message : String(e)));
  };
  useEffect(find, []);
  useEffect(() => {
    if (url) connect(url);
  }, [url]);
  useEffect(() => {
    document.title = attention > 0 ? `(${attention}) Lobotomy` : 'Lobotomy';
    window.lobotomy?.setAttention(attention);
  }, [attention]);
  // Ctrl+F searches the thread; the browser's find cannot see rows the list has not mounted.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'f') {
        event.preventDefault();
        openSearch();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
  // Links in agent text open in the system browser, never in the app window.
  useEffect(() => {
    const onClick = (event: MouseEvent) => {
      const link = (event.target as HTMLElement).closest('a');
      if (link?.href && inElectron()) {
        event.preventDefault();
        window.lobotomy!.openExternal(link.href);
      }
    };
    document.addEventListener('click', onClick);
    return () => document.removeEventListener('click', onClick);
  }, []);

  if (failed !== null) {
    return (
      <div className="splash">
        <p>没能连上后端：{failed}</p>
        <button onClick={find}>重试</button>
      </div>
    );
  }
  if (url === undefined) return <div className="splash">正在连接…</div>;
  if (url === null || !snapshot?.project) {
    return <Onboarding connected={url !== null && snapshot !== null} onBackend={setUrl} />;
  }
  return (
    <div className="app">
      <TopBar />
      <main className="main">
        <section className="conversation">
          <Thread />
          <Composer />
          {searching && <SearchBar />}
          {selectedTask && <TaskPanel taskId={selectedTask} />}
        </section>
        <Sidebar />
      </main>
      <div className="toasts">
        {toasts.map((t) => (
          <div key={t.id} className="toast">
            {t.text}
          </div>
        ))}
      </div>
    </div>
  );
}
