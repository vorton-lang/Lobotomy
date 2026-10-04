import { useState } from 'react';
import { shortSha } from '../format';
import { useStore } from '../store';
import { Settings } from './Settings';

export function TopBar() {
  const status = useStore((s) => s.status);
  const project = useStore((s) => s.snapshot?.project);
  const stopped = useStore((s) => s.snapshot?.attention.some((a) => a.kind === 'preview_stopped'));
  const [settings, setSettings] = useState(false);
  if (!project) return null;
  const repo = project.repo_path.split(/[\\/]/).filter(Boolean).at(-1);
  const behind = project.previewed !== project.integration;
  return (
    <header className="topbar">
      <strong>Lobotomy</strong>
      <span className="muted">
        {repo} · {project.branch}
      </span>
      <span className="muted" title="你的仓库所在的集成版本">
        预览 {shortSha(project.previewed)}
        {stopped ? ' · 已停止' : behind ? ' · 正在更新' : ''}
      </span>
      <span className="spacer" />
      <span className={`status ${status}`} title={status === 'open' ? '已连接后端' : '正在重新连接后端'}>
        {status === 'open' ? '已连接' : '连接中断，正在重连'}
      </span>
      <button className="ghost" onClick={() => setSettings(true)} aria-label="项目设置">
        ⚙
      </button>
      {settings && <Settings onClose={() => setSettings(false)} />}
    </header>
  );
}
