// A task's detail over the thread: what the user asked, the criteria, the latest verification,
// the candidate's changes, and the actions of its phase (data-model.md §9.2).

import { useState } from 'react';
import type { CheckRunRow, Phase, TaskDetail, VerificationRow } from '../api/types';
import { clock, PHASE_LABEL, shortSha, timeline, workRange } from '../format';
import { run, selectTask, useStore } from '../store';
import { Markdown, Modal, Output } from './common';
import { DiffPool, DiffView } from './DiffView';

export function TaskPanel({ taskId }: { taskId: string }) {
  const detail = useStore((s) => s.taskDetail);
  const task = useStore((s) => s.snapshot?.tasks.find((t) => t.id === taskId));
  const integration = useStore((s) => s.snapshot?.project?.integration);
  const [sendingBack, setSendingBack] = useState(false);
  const [editing, setEditing] = useState(false);
  if (!task) return null;
  const close = () => selectTask(null);
  const loaded = detail?.task.id === taskId ? detail : null;
  const verification = loaded?.verifications.at(-1);
  const criteria = loaded?.criteria.at(-1);
  const latest = loaded?.attempts.at(-1);
  // A verification of an earlier round stays visible, but never as the current result (#13).
  const verifiedRound = loaded?.attempts.find((a) => a.id === verification?.attempt_id)?.seq;
  const earlier = !!verification && verification.attempt_id !== latest?.id;

  // Accepting names the verification, the criteria version and the integration head the user saw;
  // the backend refuses if any moved (harness-adapter.md §4.2).
  const accept = () =>
    run('accept', {
      task_id: task.id,
      verification_id: verification?.id,
      criteria_version: task.criteria_version,
      expected_integration: integration,
    });

  return (
    <div className="task-panel" role="dialog" aria-label={task.title}>
      <header>
        <span className={`phase ${task.phase}`}>{PHASE_LABEL[task.phase]}</span>
        <h2>{task.title}</h2>
        <button className="ghost" onClick={close} aria-label="关闭">
          ✕
        </button>
      </header>
      <div className="actions">
        {task.phase === 'accepting' && verification?.state === 'passed' && (
          <>
            <button className="primary" onClick={accept}>
              验收
            </button>
            <button onClick={() => setSendingBack(true)}>退回…</button>
          </>
        )}
        {!['done', 'abandoned'].includes(task.phase) && (
          <>
            <button onClick={() => run('set_paused', { task_id: task.id, paused: !task.paused })}>{task.paused ? '恢复' : '暂停'}</button>
            <button onClick={() => run('abandon', { task_id: task.id, reason: '用户放弃' })}>放弃</button>
          </>
        )}
        {['done', 'abandoned'].includes(task.phase) && <button onClick={() => run('reopen', { task_id: task.id })}>重开</button>}
      </div>
      {!loaded ? (
        <p className="muted">正在加载…</p>
      ) : (
        <div className="panel-body">
          <p className="now">{now(task.phase, latest?.seq, verification && !earlier ? verification.state : null)}</p>
          <section>
            <h3>你的原话</h3>
            <Markdown text={loaded.task.body || '（无）'} />
          </section>
          <section>
            <h3>
              完成条件 <span className="muted">第 {criteria?.version} 版</span>
              {!['done', 'abandoned'].includes(task.phase) && (
                <button className="small" onClick={() => setEditing(true)}>
                  修改
                </button>
              )}
            </h3>
            <Markdown text={criteria?.text || '（无）'} />
          </section>
          {verification && (
            <section className={earlier ? 'earlier' : ''}>
              <h3>
                {earlier ? `上一轮的验证（第 ${verifiedRound} 轮）` : `验证（第 ${verifiedRound} 轮）`}{' '}
                <span className={`badge ${verification.state}`}>{{ running: '进行中', passed: '通过', failed: '未通过' }[verification.state]}</span>
              </h3>
              <p className="muted">
                {earlier && `第 ${latest?.seq} 轮还没有验证。`}基于集成版本 {shortSha(verification.base)} · 检查设置第 {verification.config_version} 版 ·{' '}
                {clock(verification.created_at)}
              </p>
              {verification.conflicts?.length > 0 && <p className="error">冲突：{verification.conflicts.join('、')}</p>}
              {loaded.checks
                .filter((c) => c.verification_id === verification.id)
                .map((check) => (
                  <CheckView key={check.id} check={check} earlier={earlier} />
                ))}
              {verification.state !== 'running' &&
                !verification.conflicts?.length &&
                !loaded.checks.some((c) => c.verification_id === verification.id) && (
                  <p className="muted">没有设置检查命令，验证只确认采集完整、没有冲突。可以在 ⚙ 项目设置里添加。</p>
                )}
            </section>
          )}
          <Changes detail={loaded} />
          <History detail={loaded} />
        </div>
      )}
      {sendingBack && <SendBack taskId={task.id} onClose={() => setSendingBack(false)} />}
      {editing && criteria && <EditCriteria taskId={task.id} version={criteria.version} text={criteria.text} onClose={() => setEditing(false)} />}
    </div>
  );
}

/** Where the task stands, in one line: which round, and what it waits for. */
function now(phase: Phase, round: number | undefined, verification: VerificationRow['state'] | null): string {
  switch (phase) {
    case 'queued':
      return round ? `排队中，轮到时开始第 ${round + 1} 轮执行。` : '排队中，还没有开始执行。';
    case 'executing':
      return `第 ${round} 轮执行中，还没有交出候选成果。`;
    case 'verifying':
      return verification === 'running' ? `第 ${round} 轮的候选成果正在验证。` : `第 ${round} 轮的候选成果等待验证。`;
    case 'accepting':
      return `第 ${round} 轮的候选成果通过了验证，等你验收。`;
    case 'done':
      return `已完成，验收的是第 ${round} 轮的候选成果。`;
    case 'abandoned':
      return '已放弃。';
  }
}

function CheckView({ check, earlier }: { check: CheckRunRow; earlier: boolean }) {
  const ok = !check.timed_out && check.exit_code === 0;
  return (
    <details className={`check ${ok ? 'ok' : 'failed'}`} open={!ok && !earlier}>
      <summary>
        <code>{check.command}</code> · {check.timed_out ? '超时' : `退出码 ${check.exit_code}`} · {(check.duration_ms / 1000).toFixed(1)} 秒
      </summary>
      <Output value={check.output.stdout} />
      <Output value={check.output.stderr} />
    </details>
  );
}

function Changes({ detail }: { detail: TaskDetail }) {
  const range = workRange(detail);
  if (!range) return null;
  return (
    <section className={range.earlier ? 'earlier' : ''}>
      <h3>改动</h3>
      <p className="muted">{range.label}</p>
      <DiffPool>
        <DiffView from={range.from} to={range.to} />
      </DiffPool>
    </section>
  );
}

function History({ detail }: { detail: TaskDetail }) {
  return (
    <section>
      <h3>记录</h3>
      <ol className="history">
        {timeline(detail).map((entry, i) => (
          <li key={i}>
            <span className="muted">{clock(entry.at)}</span> {entry.text}
          </li>
        ))}
      </ol>
    </section>
  );
}

function SendBack({ taskId, onClose }: { taskId: string; onClose: () => void }) {
  const [reason, setReason] = useState('');
  return (
    <Modal title="退回候选成果" onClose={onClose}>
      <label>
        理由（会原样发给执行者）
        <textarea value={reason} onChange={(e) => setReason(e.target.value)} rows={5} autoFocus />
      </label>
      <div className="actions">
        <button onClick={onClose}>取消</button>
        <button
          className="primary"
          disabled={!reason.trim()}
          onClick={async () => (await run('send_back', { task_id: taskId, reason: reason.trim() })) !== undefined && onClose()}
        >
          退回
        </button>
      </div>
    </Modal>
  );
}

function EditCriteria({ taskId, version, text, onClose }: { taskId: string; version: number; text: string; onClose: () => void }) {
  const [value, setValue] = useState(text);
  return (
    <Modal title="修改完成条件" onClose={onClose}>
      <p className="muted">修改会生成新版本。任务在执行时，新版本会发给执行者；已有的验证结果仍然对应旧版本。</p>
      <textarea value={value} onChange={(e) => setValue(e.target.value)} rows={6} autoFocus aria-label="完成条件" />
      <div className="actions">
        <button onClick={onClose}>取消</button>
        <button
          className="primary"
          disabled={value.trim() === text.trim()}
          onClick={async () => (await run('edit_criteria', { task_id: taskId, expected_version: version, text: value })) !== undefined && onClose()}
        >
          保存为第 {version + 1} 版
        </button>
      </div>
    </Modal>
  );
}
