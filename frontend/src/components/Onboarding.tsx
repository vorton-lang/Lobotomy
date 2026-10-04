// First run: connect a repository (harness-adapter.md §4.3 "接入"). The repository must be clean
// and on a branch; the backend lists what is wrong otherwise.

import { useEffect, useState } from 'react';
import { RequestFailed } from '../api/connection';
import { backendUrl, inElectron } from '../bridge';
import { call } from '../store';

export function Onboarding({ connected, onBackend }: { connected: boolean; onBackend: (url: string) => void }) {
  const [path, setPath] = useState('');
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const onboard = async (repo: string) => {
    setBusy(true);
    setError(null);
    try {
      await call('command', { name: 'onboard', args: { repo_path: repo } });
    } catch (e) {
      setError(e instanceof RequestFailed ? e.remote.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  // In Electron the backend of a new project starts after the folder is chosen; connect first.
  useEffect(() => {
    if (connected && pending) {
      setPending(null);
      void onboard(pending);
    }
  }, [connected, pending]);

  const choose = async () => {
    const repo = await window.lobotomy!.chooseRepo();
    if (!repo) return;
    setPath(repo);
    setPending(repo);
    const url = await backendUrl();
    if (url) onBackend(url);
  };

  return (
    <div className="onboarding">
      <h1>Lobotomy</h1>
      <p>选择一个本地 git 仓库。它需要检出在某个分支上，工作区干净。接入后，这个仓库成为集成版本的只读预览，Lobotomy 只会快进它的分支。</p>
      {inElectron() ? (
        <button className="primary" onClick={choose} disabled={busy || pending !== null}>
          {path ? '重新选择仓库' : '选择仓库文件夹'}
        </button>
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void onboard(path.trim());
          }}
        >
          <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="仓库路径，例如 C:\code\project" aria-label="仓库路径" />
          <button className="primary" type="submit" disabled={busy || !connected || !path.trim()}>
            接入
          </button>
        </form>
      )}
      {path && inElectron() && <p className="muted">{path}</p>}
      {(busy || pending) && <p className="muted">正在接入…</p>}
      {!connected && !inElectron() && <p className="muted">正在连接后端…</p>}
      {error && <p className="error">{error}</p>}
    </div>
  );
}
