// The project list (frontend.md §2 "M3 的布局"): 「全部」 first, then each project with how many
// items wait for the user in it and whether a turn of it runs. A project that could not be opened
// says why. Archived projects are folded at the end; 「+ 新项目」 connects another repository.

import { useState } from 'react';
import type { ProjectStatus } from '../api/types';
import { selectProject, totalAttention, useStore } from '../store';
import { Modal } from './common';
import { ConnectRepo } from './ConnectRepo';

function Count({ n }: { n: number }) {
  return n > 0 ? <span className="count">{n}</span> : null;
}

export function ProjectList() {
  const host = useStore((s) => s.host);
  const current = useStore((s) => s.project);
  const [creating, setCreating] = useState(false);
  if (!host) return null;
  // A project being connected shows once it runs.
  const listed = host.projects.filter((p) => p.state === 'running');
  const archived = host.projects.filter((p) => p.state === 'archived');
  return (
    <nav className="projects" aria-label="项目">
      <button className={`project-item ${current === null ? 'selected' : ''}`} aria-current={current === null || undefined} onClick={() => selectProject(null)}>
        <span className="project-name">全部</span>
        <Count n={totalAttention(host)} />
      </button>
      {listed.map((p) => (
        <ProjectItem key={p.id} project={p} selected={p.id === current} />
      ))}
      {archived.length > 0 && (
        <details className="archived">
          <summary className="muted">已归档 {archived.length}</summary>
          {archived.map((p) => (
            <div key={p.id} className="project-item muted" title={p.repo_path}>
              <span className="project-name">{p.name}</span>
            </div>
          ))}
        </details>
      )}
      <button className="small new-project" onClick={() => setCreating(true)}>
        + 新项目
      </button>
      {creating && (
        <Modal title="新项目" onClose={() => setCreating(false)}>
          <ConnectRepo onDone={() => setCreating(false)} />
        </Modal>
      )}
    </nav>
  );
}

function ProjectItem({ project: p, selected }: { project: ProjectStatus; selected: boolean }) {
  if (p.error !== null) {
    return (
      <div className="project-item failed" title={p.repo_path}>
        <span className="project-name">{p.name}</span>
        <span className="badge">打不开</span>
        <p className="muted failure">{p.error}</p>
      </div>
    );
  }
  return (
    <button className={`project-item ${selected ? 'selected' : ''}`} aria-current={selected || undefined} title={p.repo_path} onClick={() => selectProject(p.id)}>
      {p.busy && (
        <span className="busy" title="有 turn 在运行" aria-label="有 turn 在运行">
          ●
        </span>
      )}
      <span className="project-name">{p.name}</span>
      <Count n={p.attention.length} />
    </button>
  );
}
