// A task's detail over the thread: what the user asked, the criteria, the executor's delivery,
// and the actions of its phase (data-model.md §9.2). It is a region of
// the window, not a dialog: the thread and the sidebar stay usable while it is open. Its left edge
// drags it wider or narrower; it never gets wider than the thread.

import { useState } from 'react';
import { X } from 'lucide-react';
import type { TaskDetail } from '../api/types';
import { ACTION_LABEL, clock, deliveryReport, PHASE_LABEL, PHASE_LOOK, taskNow, timeline } from '../format';
import { run, selectTask, useStore } from '../store';
import { ICON, Markdown, Modal } from './common';
import { Sash, useWidth } from './Sash';
import { StatusPill } from './Status';

export function TaskPanel({ taskId }: { taskId: string }) {
  const column = useWidth('task-panel', 820, 360, 2400);
  const detail = useStore((s) => s.taskDetail);
  const task = useStore((s) => s.snapshot?.tasks.find((t) => t.id === taskId));
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
  const criteria = loaded?.criteria.at(-1);
  const latest = loaded?.attempts.at(-1);
  const delivery = loaded && deliveryReport(loaded.attempts);
  const acceptable = task.phase === 'accepting';
  // Messages the executor never got hold acceptance until the user decides about them (#14).
  const undelivered = loaded?.undelivered_messages ?? [];

  // Accepting names the criteria version and the undelivered messages the user saw; the backend
  // refuses if either differs. The criteria version is the one shown below, not the snapshot's,
  // which may be newer (#16).
  const accept = (dropping: string[]) => {
    if (!criteria) return;
    return run('accept', { task_id: task.id, criteria_version: criteria.version, dropping });
  };

  return (
    <section className="task-panel" aria-label={task.title} style={{ width: `min(${column.width}px, 100%)` }}>
      <Sash label="任务面板的宽度" edge="left" column={column} />
      <header>
        <StatusPill look={PHASE_LOOK[task.phase]} className={`phase ${task.phase}`}>
          {PHASE_LABEL[task.phase]}
        </StatusPill>
        <h2>{task.title}</h2>
        <button className="icon" onClick={close} aria-label="关闭">
          <X {...ICON} />
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
            <button onClick={() => run('set_paused', { task_id: task.id, paused: !task.paused })}>
              {task.paused ? '恢复' : '暂停'}
            </button>
            <button onClick={() => run('abandon', { task_id: task.id, reason: '用户放弃' })}>放弃</button>
          </>
        )}
      </div>
      {task.paused && (
        <p className="muted">
          {task.paused_by === 'manager' ? 'Angela 暂停了这个任务。' : '你暂停了这个任务，只有你能恢复。'}
        </p>
      )}
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
          <p className="now">{taskNow(task.phase, latest?.seq)}</p>
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
            <h3>
              {delivery?.earlier
                ? `上一轮交付说明（第 ${delivery.round} 轮）`
                : `本轮交付${latest ? `（第 ${latest.seq} 轮）` : ''}`}
            </h3>
            {delivery?.earlier && <p className="muted">本轮还没有交付说明，以下是旧轮说明。</p>}
            {delivery ? (
              <>
                {delivery.pending && <p className="muted">执行者已提交说明，成果尚未固定。</p>}
                <Markdown key={`${delivery.round}:${delivery.text}`} text={delivery.text} />
              </>
            ) : (
              <p className="muted">本轮还没有交付说明。</p>
            )}
          </section>
          {loaded.task.assignment ? (
            // Angela's task: the user's words she quoted, then her own (roles-and-tasks.md §4).
            <>
              <section>
                <h3>
                  你的原话 <span className="muted">Angela 引用</span>
                </h3>
                <Markdown text={loaded.task.body || '（Angela 没有引用你的原话）'} />
              </section>
              <section>
                <h3>Angela 的解读</h3>
                <Markdown text={loaded.task.assignment.interpretation} />
                <p>
                  允许的动作：{loaded.task.assignment.actions.map((a) => ACTION_LABEL[a]).join('、')}
                  <br />
                  停止条件：{loaded.task.assignment.stop_when}
                </p>
              </section>
            </>
          ) : (
            <section>
              <h3>你的原话</h3>
              <Markdown text={loaded.task.body || '（无）'} />
            </section>
          )}
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
          <History detail={loaded} />
        </div>
      )}
      {sendingBack !== null && <SendBack taskId={task.id} initial={sendingBack} onClose={() => setSendingBack(null)} />}
      {editing && criteria && (
        <EditCriteria
          taskId={task.id}
          version={criteria.version}
          text={criteria.text}
          onClose={() => setEditing(false)}
        />
      )}
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
          onClick={async () =>
            (await run('send_back', { task_id: taskId, reason: reason.trim() })) !== undefined && onClose()
          }
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
      <p className="muted">修改会生成新版本。任务在执行时，新版本会发给执行者。</p>
      <textarea value={value} onChange={(e) => setValue(e.target.value)} rows={6} autoFocus aria-label="完成条件" />
      <div className="actions">
        <button onClick={onClose}>取消</button>
        <button
          className="primary"
          disabled={value.trim() === text.trim()}
          onClick={async () =>
            (await run('edit_criteria', { task_id: taskId, expected_version: version, text: value })) !== undefined &&
            onClose()
          }
        >
          保存为第 {version + 1} 版
        </button>
      </div>
    </Modal>
  );
}
