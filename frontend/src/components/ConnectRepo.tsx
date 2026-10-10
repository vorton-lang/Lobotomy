// Connecting a repository as a new project (harness-adapter.md §4.3 "接入"; data-model.md §10.4),
// while the backend runs. The repository must be clean and on a branch; the backend lists what is
// wrong otherwise. The window then shows the new project.

import { useState } from 'react';
import { RequestFailed } from '../api/connection';
import type { ProjectStatus } from '../api/types';
import { inElectron } from '../bridge';
import { call, refreshHost, selectProject } from '../store';

export function ConnectRepo({ connected = true, onDone }: { connected?: boolean; onDone?: () => void }) {
  const [path, setPath] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const create = async (repo: string) => {
    setBusy(true);
    setError(null);
    try {
      const entry = await call<Pick<ProjectStatus, 'id'>>('command', { name: 'create_project', args: { repo_path: repo } });
      await refreshHost();
      await selectProject(entry.id);
      onDone?.();
    } catch (e) {
      setError(e instanceof RequestFailed ? e.remote.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const choose = async () => {
    const repo = await window.lobotomy!.chooseRepo();
    if (!repo) return;
    setPath(repo);
    await create(repo);
  };

  return (
    <div className="connect-repo">
      <p>选择一个本地 git 仓库。它需要检出在某个分支上，工作区干净。接入后，这个仓库成为集成版本的只读预览，Lobotomy 只会快进它的分支。</p>
      {inElectron() ? (
        <button className="primary" onClick={choose} disabled={busy || !connected}>
          {path ? '重新选择仓库' : '选择仓库文件夹'}
        </button>
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void create(path.trim());
          }}
        >
          <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="仓库路径，例如 C:\code\project" aria-label="仓库路径" />
          <button className="primary" type="submit" disabled={busy || !connected || !path.trim()}>
            接入
          </button>
        </form>
      )}
      {path && inElectron() && <p className="muted">{path}</p>}
      {busy && <p className="muted">正在接入…</p>}
      {!connected && <p className="muted">正在连接后端…</p>}
      {error && <p className="error">{error}</p>}
    </div>
  );
}
