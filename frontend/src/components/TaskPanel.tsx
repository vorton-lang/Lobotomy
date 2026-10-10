// A task's detail over the thread: what the user asked, the criteria, the latest verification,
// the candidate's changes, and the actions of its phase (data-model.md §9.2). It is a region of
// the window, not a dialog: the thread and the sidebar stay usable while it is open.

import { useState } from 'react';
import type { CheckRun, TaskDetail } from '../api/types';
import { checkPassed, checkSummary, clock, deliveryReport, PHASE_LABEL, shortSha, taskNow, timeline, VERIFICATION_LABEL, workRange } from '../format';
import { run, selectTask, useStore } from '../store';
import { Markdown, Modal, Output } from './common';
import { DiffPool, DiffView } from './DiffView';
import { TrialControls } from './TrialControls';

export function TaskPanel({ taskId }: { taskId: string }) {
  const detail = useStore((s) => s.taskDetail);
  const task = useStore((s) => s.snapshot?.tasks.find((t) => t.id === taskId));
  const integration = useStore((s) => s.snapshot?.project?.integration);
  // The task its executor works on now, when that is another one: after acceptance the next task
  // starts while this panel stays open (#18).
  const current = useStore((s) => {
    const executor = s.snapshot?.roles.find((r) => r.name === task?.executor);
    if (!executor?.task_id || executor.task_id === taskId) return undefined;
    return s.snapshot?.tasks.find((t) => t.id === executor.task_id);
  });
  // The reason to start the send-back dialog with, or `null` while it is closed.
  const [sendingBack, setSendingBack] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  if (!task) return null;
  const close = () => selectTask(null);
  const closed = task.phase === 'done' || task.phase === 'abandoned';
  const loaded = detail?.task.id === taskId ? detail : null;
  const verification = loaded?.verifications.at(-1);
  const criteria = loaded?.criteria.at(-1);
  const latest = loaded?.attempts.at(-1);
  const delivery = loaded && deliveryReport(loaded.attempts);
  // A verification of an earlier round stays visible, but never as the current result (#13).
  const verifiedRound = loaded?.attempts.find((a) => a.id === verification?.attempt_id)?.seq;
  const earlier = !!verification && verification.attempt_id !== latest?.id;
  const acceptable = task.phase === 'accepting' && verification?.state === 'passed';
  // Messages the executor never got hold acceptance until the user decides about them (#14).
  const undelivered = loaded?.undelivered_messages ?? [];

  // Accepting names the verification, the criteria version, the integration head and the
  // undelivered messages the user saw; the backend refuses if any differ (harness-adapter.md §4.2).
  // The criteria version is the one shown below, not the snapshot's, which may be newer (#16).
  const accept = (dropping: string[]) =>
    run('accept', {
      task_id: task.id,
      verification_id: verification?.id,
      criteria_version: criteria?.version,
      expected_integration: integration,
      dropping,
    });

  return (
    <section className="task-panel" aria-label={task.title}>
      <header>
        <span className={`badge phase ${task.phase}`}>{PHASE_LABEL[task.phase]}</span>
        <h2>{task.title}</h2>
        <button className="ghost" onClick={close} aria-label="关闭">
          ✕
        </button>
      </header>
      <div className="actions">
        {acceptable && loaded && undelivered.length === 0 && (
          <>
            <button className="primary" onClick={() => accept([])}>
              验收
            </button>
            <button onClick={() => setSendingBack('')}>退回…</button>
          </>
        )}
        {closed ? (
          <button onClick={() => run('reopen', { task_id: task.id })}>重开</button>
        ) : (
          <>
            <button onClick={() => run('set_paused', { task_id: task.id, paused: !task.paused })}>{task.paused ? '恢复' : '暂停'}</button>
            <button onClick={() => run('abandon', { task_id: task.id, reason: '用户放弃' })}>放弃</button>
          </>
        )}
      </div>
      {current && (
        <p className="now-running">
          {task.executor} 现在做的是「{current.title}」（{PHASE_LABEL[current.phase]}）。
          <button className="link" onClick={() => selectTask(current.id)}>
            查看当前任务
          </button>
        </p>
      )}
      {!loaded ? (
        <p className="muted">正在加载…</p>
      ) : (
        <div className="panel-body">
          <p className="now">{taskNow(task.phase, latest?.seq, verification && !earlier ? verification.state : null)}</p>
          {acceptable && undelivered.length > 0 && (
            <section className="card warn undelivered">
              <h3>还有 {undelivered.length} 条消息没有交给执行者</h3>
              <p className="muted">它们排在执行者交出候选成果之后，候选成果没有处理它们。验收后，它们不会再投递。</p>
              {undelivered.map((m) => (
                <div key={m.id} className="quote">
                  <Markdown text={m.body} />
                </div>
              ))}
              <div className="actions">
                <button className="primary" onClick={() => setSendingBack('请按我补充的要求修改（见上面的消息）。')}>
                  退回并交给执行者…
                </button>
                <button onClick={() => accept(undelivered.map((m) => m.id))}>验收，不再投递这些消息</button>
              </div>
            </section>
          )}
          <section className={`delivery${delivery?.earlier ? ' earlier' : ''}`} aria-label="交付说明">
            <h3>{delivery?.earlier ? `上一轮交付说明（第 ${delivery.round} 轮）` : `本轮交付${latest ? `（第 ${latest.seq} 轮）` : ''}`}</h3>
            {delivery?.earlier && <p className="muted">本轮还没有交付说明，以下是旧轮说明。</p>}
            {delivery ? (
              <>
                {delivery.pending && <p className="muted">执行者已提交说明，成果尚未固定。</p>}
                <Markdown key={`${delivery.round}:${delivery.text}`} text={delivery.text} />
              </>
            ) : <p className="muted">本轮还没有交付说明。</p>}
          </section>
          <TrialControls key={task.id} detail={loaded} />
          <section>
            <h3>你的原话</h3>
            <Markdown text={loaded.task.body || '（无）'} />
          </section>
          <section>
            <h3>
              完成条件 <span className="muted">第 {criteria?.version} 版</span>
              {!closed && (
                <button className="small" onClick={() => setEditing(true)}>
                  修改
                </button>
              )}
            </h3>
            <Markdown text={criteria?.text || '（无）'} />
          </section>
          {verification && (
            <details className={`verification-details${earlier ? ' earlier' : ''}`}>
              <summary>
                {earlier ? `上一轮的验证（第 ${verifiedRound} 轮）` : `验证（第 ${verifiedRound} 轮）`}{' '}
                <span className={`badge ${verification.state}`}>{VERIFICATION_LABEL[verification.state]}</span>
              </summary>
              <p className="muted">
                {earlier && `第 ${latest?.seq} 轮还没有验证。`}基于集成版本 {shortSha(verification.base)} · 检查设置第 {verification.config_version} 版 ·{' '}
                {clock(verification.created_at)}
              </p>
              {verification.conflicts.length > 0 && <p className="error">冲突：{verification.conflicts.join('、')}</p>}
              {loaded.checks
                .filter((c) => c.verification_id === verification.id)
                .map((check) => (
                  <CheckView key={check.id} check={check} earlier={earlier} />
                ))}
              {verification.state !== 'running' &&
                verification.conflicts.length === 0 &&
                !loaded.checks.some((c) => c.verification_id === verification.id) && (
                  <p className="muted">没有设置检查命令，验证只确认采集完整、没有冲突。可以在 ⚙ 设置里添加。</p>
                )}
            </details>
          )}
          <Changes detail={loaded} />
          <History detail={loaded} />
        </div>
      )}
      {sendingBack !== null && <SendBack taskId={task.id} initial={sendingBack} onClose={() => setSendingBack(null)} />}
      {editing && criteria && <EditCriteria taskId={task.id} version={criteria.version} text={criteria.text} onClose={() => setEditing(false)} />}
    </section>
  );
}

function CheckView({ check, earlier }: { check: CheckRun; earlier: boolean }) {
  const ok = checkPassed(check);
  return (
    <details className={`check ${ok ? 'ok' : 'failed'}`} open={!ok && !earlier}>
      <summary>
        <code>{check.command}</code> · {checkSummary(check)}
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
    <details className={`changes-details${range.earlier ? ' earlier' : ''}`}>
      <summary>改动 · {range.label}</summary>
      <p className="muted">{range.label}</p>
      <DiffPool>
        <DiffView from={range.from} to={range.to} />
      </DiffPool>
    </details>
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

function SendBack({ taskId, initial, onClose }: { taskId: string; initial: string; onClose: () => void }) {
  const [reason, setReason] = useState(initial);
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

function EditCriteria(props: { taskId: string; version: number; text: string; onClose: () => void }) {
  // The version being edited is the one the dialog opened with; a newer one saved meanwhile
  // makes the backend refuse, instead of being written over (#16).
  const [{ version, text }] = useState(() => ({ version: props.version, text: props.text }));
  const { taskId, onClose } = props;
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
