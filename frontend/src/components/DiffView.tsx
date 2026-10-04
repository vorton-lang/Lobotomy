// The changes between two commits, file by file, with @pierre/diffs (frontend.md §4.1).

import { useEffect, useState } from 'react';
import { MultiFileDiff, WorkerPoolContextProvider } from '@pierre/diffs/react';
import type { Content, FileChange } from '../api/types';
import { bytes } from '../format';
import { call, toast } from '../store';

const theme = { dark: 'pierre-dark', light: 'pierre-light' } as const;
const poolOptions = {
  workerFactory: () => new Worker(new URL('@pierre/diffs/worker/worker.js', import.meta.url), { type: 'module' }),
  poolSize: 2,
};

export function DiffPool({ children }: { children: React.ReactNode }) {
  return (
    <WorkerPoolContextProvider poolOptions={poolOptions} highlighterOptions={{ theme }}>
      {children}
    </WorkerPoolContextProvider>
  );
}

function describe(content: Content | null): string | null {
  if (!content) return null;
  switch (content.kind) {
    case 'binary':
      return `二进制文件，${bytes(content.size)}`;
    case 'too_large':
      return `文件太大（超过 ${bytes(content.size - 1)}），不显示内容`;
    case 'symlink':
      return `符号链接 → ${content.target}`;
    case 'conflict':
      return '有冲突';
    case 'text':
      return null;
  }
}

const text = (content: Content | null) => (content?.kind === 'text' ? content.text : '');

export function DiffView({ from, to }: { from: string; to: string }) {
  const [changes, setChanges] = useState<FileChange[] | null>(null);
  useEffect(() => {
    let current = true;
    call<FileChange[]>('diff', { from, to })
      .then((c) => current && setChanges(c))
      .catch((e) => toast(String(e)));
    return () => {
      current = false;
    };
  }, [from, to]);
  if (changes === null) return <p className="muted">正在计算改动…</p>;
  if (changes.length === 0) return <p className="muted">没有改动。</p>;
  return (
    <div className="diff">
      <p className="muted">{changes.length} 个文件</p>
      {changes.map((change) => {
        const note = describe(change.old) ?? describe(change.new);
        if (note) {
          return (
            <div key={change.path} className="diff-note">
              <code>{change.path}</code> · {note}
            </div>
          );
        }
        return (
          <MultiFileDiff
            key={change.path}
            oldFile={{ name: change.path, contents: text(change.old), cacheKey: `${from}:${change.path}` }}
            newFile={{ name: change.path, contents: text(change.new), cacheKey: `${to}:${change.path}` }}
            options={{ diffStyle: 'unified', theme, overflow: 'wrap' }}
          />
        );
      })}
    </div>
  );
}
