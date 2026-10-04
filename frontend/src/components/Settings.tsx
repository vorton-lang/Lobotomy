// Project configuration (data-model.md §9.1 project_config): check commands, capture scope and
// the size guardrail. Each save is a new version; checks and captures name the version they used.

import { useState } from 'react';
import type { Check, ProjectConfig } from '../api/types';
import { run, useStore } from '../store';
import { Modal } from './common';

const lines = (text: string) =>
  text
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);

export function Settings({ onClose }: { onClose: () => void }) {
  const current = useStore((s) => s.snapshot?.config);
  const [checks, setChecks] = useState<Check[]>(current?.config.checks ?? []);
  const [excluded, setExcluded] = useState((current?.config.excluded ?? []).join('\n'));
  const [forced, setForced] = useState((current?.config.force_tracked ?? []).join('\n'));
  const [maxFiles, setMaxFiles] = useState(current?.config.max_new_files ?? 1000);
  const [maxMb, setMaxMb] = useState(Math.round((current?.config.max_new_bytes ?? 50 * 1024 * 1024) / 1024 / 1024));
  if (!current) return null;

  const save = async () => {
    const config: ProjectConfig = {
      checks: checks.filter((c) => c.command.trim()),
      excluded: lines(excluded),
      force_tracked: lines(forced),
      max_new_files: maxFiles,
      max_new_bytes: maxMb * 1024 * 1024,
    };
    const saved = await run('edit_project_config', { expected_version: current.version, config });
    if (saved !== undefined) onClose();
  };
  const setCheck = (i: number, patch: Partial<Check>) => setChecks(checks.map((c, j) => (i === j ? { ...c, ...patch } : c)));

  return (
    <Modal title={`项目设置 · 第 ${current.version} 版`} onClose={onClose} wide>
      <h3>检查命令</h3>
      <p className="muted">在验证现场按顺序运行，退出码为 0 算通过。第一条失败后不再运行其余的。</p>
      {checks.map((check, i) => (
        <div key={i} className="check-row">
          <input value={check.command} onChange={(e) => setCheck(i, { command: e.target.value })} placeholder="例如 cargo test" aria-label="检查命令" />
          <input
            type="number"
            min={1}
            value={check.timeout_secs}
            onChange={(e) => setCheck(i, { timeout_secs: Number(e.target.value) })}
            aria-label="超时（秒）"
          />
          <span className="muted">秒</span>
          <button className="ghost" onClick={() => setChecks(checks.filter((_, j) => j !== i))} aria-label="删除">
            ✕
          </button>
        </div>
      ))}
      <button className="small" onClick={() => setChecks([...checks, { command: '', timeout_secs: 600 }])}>
        + 添加检查命令
      </button>

      <h3>采集范围</h3>
      <label>
        始终排除（gitignore 写法，每行一条；重新物化时作为缓存保留）
        <textarea value={excluded} onChange={(e) => setExcluded(e.target.value)} rows={3} placeholder={'target/\nnode_modules/'} />
      </label>
      <label>
        强制采集（路径前缀，每行一条；即使被 ignore 规则匹配也采集）
        <textarea value={forced} onChange={(e) => setForced(e.target.value)} rows={2} />
      </label>

      <h3>体积护栏</h3>
      <p className="muted">一次采集新增的文件超过任一上限时，采集停下，等你决定。</p>
      <div className="check-row">
        <input type="number" min={0} value={maxFiles} onChange={(e) => setMaxFiles(Number(e.target.value))} aria-label="新增文件数上限" />
        <span className="muted">个文件</span>
        <input type="number" min={0} value={maxMb} onChange={(e) => setMaxMb(Number(e.target.value))} aria-label="新增大小上限" />
        <span className="muted">MB</span>
      </div>

      <div className="actions">
        <button onClick={onClose}>取消</button>
        <button className="primary" onClick={save}>
          保存为第 {current.version + 1} 版
        </button>
      </div>
    </Modal>
  );
}
