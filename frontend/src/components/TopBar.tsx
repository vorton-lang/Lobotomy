import { Settings } from 'lucide-react';
import { closeSettings, openSettings, useStore } from '../store';
import { ICON } from './common';

export function TopBar() {
  const status = useStore((s) => s.status);
  const all = useStore((s) => s.project === null);
  const project = useStore((s) => s.snapshot?.project);
  const settings = useStore((s) => s.settings !== null);
  const repo = project?.repo_path.split(/[\\/]/).filter(Boolean).at(-1);
  return (
    <header className="topbar">
      <strong>Lobotomy</strong>
      {all ? <span className="muted">全部项目</span> : project && <span className="muted">{repo}</span>}
      <span className="spacer" />
      <span className={`status ${status}`} title={status === 'open' ? '已连接后端' : '正在重新连接后端'}>
        {status === 'open' ? '已连接' : '连接中断，正在重连'}
      </span>
      <button
        className="icon"
        onClick={settings ? closeSettings : openSettings}
        aria-label="设置"
        aria-pressed={settings}
        title="设置"
      >
        <Settings {...ICON} />
      </button>
    </header>
  );
}
