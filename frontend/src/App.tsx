// The window (frontend.md §2 "M3 的布局"): the project list on the left. 「全部」 lists what waits for
// the user in every project. A project shows as in M1: the executor's thread in the main area, what
// waits for the user and the Workboard on the right, a task's detail in a panel over the thread.

import { useEffect, useState } from 'react';
import { backendUrl, inElectron } from './bridge';
import { Composer } from './components/Composer';
import { Onboarding } from './components/Onboarding';
import { Overview } from './components/Overview';
import { ProjectList } from './components/ProjectList';
import { SearchBar } from './components/SearchBar';
import { Sidebar } from './components/Sidebar';
import { TaskPanel } from './components/TaskPanel';
import { Thread } from './components/Thread';
import { TopBar } from './components/TopBar';
import { connect, openSearch, totalAttention, useStore } from './store';

export function App() {
  const [url, setUrl] = useState<string | null | undefined>(undefined);
  // Why the backend could not be reached at start, such as a backend that did not start (#16).
  const [failed, setFailed] = useState<string | null>(null);
  // Every project's events refresh the host snapshot; the window takes only what it shows from
  // it, so a refresh does not render the thread again.
  const connected = useStore((s) => s.host !== null);
  // No project yet, or only one being connected, which shows once it runs.
  const empty = useStore((s) => s.host !== null && s.host.projects.every((p) => p.state === 'onboarding'));
  const project = useStore((s) => s.project);
  const opened = useStore((s) => s.snapshot?.project != null);
  const selectedTask = useStore((s) => s.selectedTask);
  const toasts = useStore((s) => s.toasts);
  const searching = useStore((s) => s.search !== null);
  // The title and the tray count every project's items (data-model.md §10.5).
  const attention = useStore((s) => totalAttention(s.host));

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
  if (url === null || !connected || empty) return <Onboarding connected={connected} />;
  return (
    <div className="app">
      <TopBar />
      <div className="body">
        <ProjectList />
        <main className="main">
          {project === null ? (
            <Overview />
          ) : !opened ? (
            <div className="splash">正在打开项目…</div>
          ) : (
            <>
              <section className="conversation">
                <Thread />
                <Composer />
                {searching && <SearchBar />}
                {selectedTask && <TaskPanel taskId={selectedTask} />}
              </section>
              <Sidebar />
            </>
          )}
        </main>
      </div>
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
