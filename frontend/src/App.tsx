// The M1 window (frontend.md §2 "M1 的布局"): Malkuth's thread in the main area, what waits for
// the user and the Workboard on the right, a task's detail in a panel over the thread.

import { useEffect, useState } from 'react';
import { backendUrl, inElectron } from './bridge';
import { Composer } from './components/Composer';
import { Onboarding } from './components/Onboarding';
import { Sidebar } from './components/Sidebar';
import { TaskPanel } from './components/TaskPanel';
import { Thread } from './components/Thread';
import { TopBar } from './components/TopBar';
import { connect, useStore } from './store';

export function App() {
  const [url, setUrl] = useState<string | null | undefined>(undefined);
  const snapshot = useStore((s) => s.snapshot);
  const selectedTask = useStore((s) => s.selectedTask);
  const toasts = useStore((s) => s.toasts);
  const attention = snapshot?.attention.length ?? 0;

  useEffect(() => {
    void backendUrl().then(setUrl);
  }, []);
  useEffect(() => {
    if (url) connect(url);
  }, [url]);
  useEffect(() => {
    document.title = attention > 0 ? `(${attention}) Lobotomy` : 'Lobotomy';
    window.lobotomy?.setAttention(attention);
  }, [attention]);
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
