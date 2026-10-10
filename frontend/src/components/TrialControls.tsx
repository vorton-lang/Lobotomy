// Desktop terminals are runtime state, polled only while the task panel is open. A newer
// attempt changes the launch recipe, never an already opened terminal or its candidate copy.

import { useEffect, useRef, useState } from 'react';
import type { Attempt, TaskDetail, TrialView } from '../api/types';
import { shortSha, trialCandidate } from '../format';
import { call, useCommand, useStore } from '../store';

export function TrialControls({ detail }: { detail: TaskDetail }) {
  const taskId = detail.task.id;
  const status = useStore((s) => s.status);
  const [trials, setTrials] = useState<TrialView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A read started before a launch/stop completed must not overwrite that command's result.
  const revision = useRef(0);
  useEffect(() => {
    if (status !== 'open') return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      const before = revision.current;
      try {
        const result = await call<TrialView[]>('trials', { task_id: taskId });
        if (!disposed && before === revision.current) {
          setTrials(result);
          setError(null);
        }
      } catch (e) {
        if (!disposed && before === revision.current) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!disposed) timer = setTimeout(refresh, 2_000);
      }
    };
    void refresh();
    return () => { disposed = true; clearTimeout(timer); };
  }, [taskId, status]);

  const update = (trial: TrialView) => {
    revision.current++;
    setTrials((previous) => [...(previous ?? []).filter((item) => item.id !== trial.id), trial]);
    setError(null);
  };
  const latest = detail.attempts.at(-1);
  const candidate = trialCandidate(detail);
  const opened = trials?.some((trial) => trial.attempt_id === latest?.id && trial.state === 'open') ?? false;

  return (
    <section className="trial-controls" aria-label="一键体验">
      <h3>体验本轮成果{latest ? `（第 ${latest.seq} 轮）` : ''}</h3>
      {latest?.trial ? (
        <>
          {latest.trial.purpose && <p>{latest.trial.purpose}</p>}
          <p className="muted">候选版本：{candidate ? <code title={candidate}>{shortSha(candidate)}</code> : '尚未固定'}</p>
          <pre className="trial-command"><code>{latest.trial.command}</code></pre>
          <p className="muted">点击后会在后端所在电脑的桌面终端中运行命令，输入和输出都在该终端中。终端打开不代表应用已就绪，请查看终端输出。</p>
          <LaunchTrial
            key={`${latest.id}:${candidate}`}
            attempt={latest}
            candidate={candidate}
            disabled={status !== 'open' || trials === null || error !== null || opened}
            opened={opened}
            onStarted={update}
          />
          {!candidate && <p className="muted">候选成果尚未固定，固定后才能体验。</p>}
        </>
      ) : <p className="muted">本轮未提供体验命令。</p>}
      {status !== 'open' ? <p className="muted">连接恢复后才能更新体验状态。</p>
        : error ? <p className="error" role="status">无法读取体验状态：{error}</p>
        : trials === null && <p className="muted">正在读取体验状态…</p>}
      {!!trials?.length && (
        <div className="trial-list" aria-label="已启动的体验">
          {trials.map((trial) => (
            <TrialTerminal
              key={trial.id}
              trial={trial}
              earlier={trial.attempt_id !== latest?.id}
              connected={status === 'open'}
              onStopped={() => update({ ...trial, state: 'closed' })}
            />
          ))}
        </div>
      )}
    </section>
  );
}

function LaunchTrial({ attempt, candidate, disabled, opened, onStarted }: {
  attempt: Attempt;
  candidate: string | null;
  disabled: boolean;
  opened: boolean;
  onStarted: (trial: TrialView) => void;
}) {
  const { submit, busy } = useCommand<TrialView>('start_trial');
  const launch = async () => {
    if (!candidate || disabled) return;
    const result = await submit({ task_id: attempt.task_id, attempt_id: attempt.id, expected_candidate: candidate });
    if (result !== undefined) onStarted(result);
  };
  return (
    <button className="primary" disabled={disabled || busy || !candidate} onClick={launch}>
      {busy ? '正在打开终端…' : opened ? '本轮终端已打开' : '一键体验'}
    </button>
  );
}

function TrialTerminal({ trial, earlier, connected, onStopped }: {
  trial: TrialView;
  earlier: boolean;
  connected: boolean;
  onStopped: () => void;
}) {
  const { submit, busy } = useCommand<null>('stop_trial');
  const stop = async () => {
    if ((await submit({ trial_id: trial.id })) !== undefined) onStopped();
  };
  return (
    <article className={`card trial-terminal${earlier ? ' earlier' : ''}`} aria-label={`第 ${trial.round} 轮体验`}>
      <p>
        <strong>第 {trial.round} 轮 · <code title={trial.candidate}>{shortSha(trial.candidate)}</code></strong>{' '}
        {earlier && <span className="badge">旧轮体验</span>}{' '}
        <span className="badge">{trial.state === 'open' ? '终端已打开' : '终端已关闭'}</span>
      </p>
      {earlier && trial.state === 'open' && <p className="muted">仍在旧轮副本中运行，不会自动切换到本轮。</p>}
      {trial.purpose && <p>{trial.purpose}</p>}
      <pre className="trial-command"><code>{trial.command}</code></pre>
      <p className="muted">运行目录：<code>{trial.directory}</code></p>
      {trial.error && <p className="error">{trial.error}</p>}
      {trial.state === 'open' && <button disabled={!connected || busy} onClick={stop}>{busy ? '正在停止…' : '停止体验'}</button>}
    </article>
  );
}
