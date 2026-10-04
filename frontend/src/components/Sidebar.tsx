// The right column (frontend.md §2): what waits for the user, then the Workboard.

import { useState } from 'react';
import type { Attention, Capture, Hold, RoleView, Task } from '../api/types';
import { bytes, clock, elapsed, HARNESS_LABEL, PERMISSION_LABEL, PHASE_LABEL } from '../format';
import { act, MAIN_ROLE, run, selectTask, setPermission, toast, useStore } from '../store';
import { Modal, useNow } from './common';
import { DiffPool, DiffView } from './DiffView';

export function Sidebar() {
  const attention = useStore((s) => s.snapshot?.attention ?? []);
  return (
    <aside className="sidebar">
      <section className="attention">
        <h3>
          等你决定 {attention.length > 0 && <span className="count">{attention.length}</span>}
        </h3>
        {attention.length === 0 && <p className="muted">现在没有需要你处理的事。</p>}
        {attention.map((a, i) => (
          <AttentionCard key={i} attention={a} />
        ))}
      </section>
      <Workboard />
    </aside>
  );
}

function AttentionCard({ attention: a }: { attention: Attention }) {
  switch (a.kind) {
    case 'hold':
      return <HoldCard role={a.role} hold={a.hold} />;
    case 'outside_changes':
      return <OutsideChangesCard role={a.role} capture={a.capture} />;
    case 'unknown_turn':
      return (
        <div className="card warn">
          <p>后端重启前启动的 {a.role} 的 CLI 仍在运行。运行时不会自动结束它。</p>
          <div className="actions">
            <button onClick={() => act('terminate', { turn_id: a.turn_id })}>终止残留进程</button>
          </div>
        </div>
      );
    case 'task_blocked':
      return <BlockedCard taskId={a.task_id} title={a.title} reason={a.reason} />;
    case 'stalled':
      return <StalledCard attention={a} />;
    case 'accept':
      return (
        <div className="card">
          <p>
            「{a.title}」通过了验证，等你验收。
          </p>
          <UndeliveredNote taskId={a.task_id} />
          <div className="actions">
            <button className="primary" onClick={() => selectTask(a.task_id)}>
              查看并验收
            </button>
          </div>
        </div>
      );
    case 'quota':
      return (
        <div className="card warn">
          <p>
            额度不足（{a.domain.harness}）
            {a.domain.resets_at ? ` · ${clock(a.domain.resets_at)} 重置` : ' · 重置时间未知'} · 等待手动重试
          </p>
          {a.domain.message && <p className="muted">{a.domain.message}</p>}
          <div className="actions">
            <button onClick={() => act('quota_retry', { harness: a.domain.harness })}>额度重试</button>
          </div>
        </div>
      );
    case 'job_failed':
      return (
        <div className="card warn">
          <p>运行时的工作出错：{a.key}</p>
          <p className="muted">{a.reason}</p>
          <div className="actions">
            <button onClick={() => act('retry_failed')}>重试</button>
          </div>
        </div>
      );
    case 'preview_stopped':
      return (
        <div className="card warn">
          <p>预览已停止：{a.reason}</p>
          <p className="muted">把仓库恢复到上次预览的状态后重试。Lobotomy 不会覆盖你的文件。</p>
          <div className="actions">
            <button onClick={() => run('retry_preview')}>重试预览</button>
          </div>
        </div>
      );
  }
}

function HoldCard({ role, hold }: { role: string; hold: Hold }) {
  const roleView = useStore((s) => s.snapshot?.roles.find((r) => r.name === role));
  const harness = useStore((s) => s.snapshot?.harnesses.find((h) => h.harness === roleView?.harness));
  const taskId = roleView?.task_id;
  const proceed = () => run('continue', { role });
  const newSession = () => run('new_native_session', { role });
  const abandon = taskId ? () => run('abandon', { task_id: taskId, reason: '用户在异常后放弃' }) : undefined;
  if (hold.kind === 'abnormal') {
    const failure = hold.turn.failure;
    const why =
      hold.turn.outcome === 'interrupted'
        ? '被中断'
        : failure?.unstarted
          ? '没能启动'
          : failure?.kind === 'quota'
            ? '因额度不足而失败'
            : '失败';
    const harnessName = HARNESS_LABEL[roleView?.harness ?? ''] ?? roleView?.harness;
    // The environment refused full access: the one-step way on is auto review (harness-adapter.md §1.9).
    const switchable = failure?.kind === 'permission' && harness?.permission === 'full';
    const switchAndProceed = async () => {
      if (!harness || (await setPermission(harness.harness, 'auto_review')) === undefined) return;
      await proceed();
    };
    return (
      <div className="card warn">
        <p>
          {role} 的上一个 turn {why}，它停下等你决定。
        </p>
        {failure?.kind === 'permission' && harness && (
          <p>
            这个环境的管理设置不允许 {harnessName} 以「{PERMISSION_LABEL[harness.permission]}」运行。
            {switchable ? '可以改用自动审批：在沙箱里工作，超出沙箱的操作由自动审核决定。' : '可以在 ⚙ 设置里换一种权限模式，或请管理员调整。'}
          </p>
        )}
        {failure?.unstarted && <p className="muted">{harnessName} 没有收到那次的内容；继续时会原样重新发送。</p>}
        {failure?.kind === 'permission' ? (
          <details>
            <summary className="muted">原始错误</summary>
            <p className="muted failure">{failure.message}</p>
          </details>
        ) : (
          failure && <p className="muted failure">{failure.message}</p>
        )}
        <div className="actions">
          {switchable && (
            <button className="primary" onClick={switchAndProceed}>
              改用自动审批并继续
            </button>
          )}
          <button className={switchable ? '' : 'primary'} onClick={proceed}>
            继续
          </button>
          <button onClick={newSession}>新会话</button>
          {abandon && <button onClick={abandon}>放弃任务</button>}
        </div>
      </div>
    );
  }
  if (hold.kind === 'session_unidentified') {
    return (
      <div className="card warn">
        <p>{role} 的会话没有记下 Codex 的会话 ID，无法接着上次继续。</p>
        <div className="actions">
          <button className="primary" onClick={newSession}>
            新会话
          </button>
        </div>
      </div>
    );
  }
  const c = hold.capture;
  const files = c.detail?.files ?? [];
  const paths = c.detail?.paths ?? [];
  const decide = (name: string) => run(name, { capture_id: c.id });
  return (
    <div className="card warn">
      {c.state === 'oversized' ? (
        <p>
          {role} 的这次采集新增了 {files.length} 个文件，共 {bytes(c.detail?.total_bytes ?? 0)}，超过了项目设定的上限。
          {c.kind === 'candidate' && ' 它报告的 done 还没有生效。'}
        </p>
      ) : (
        <p>
          {role} 的工作目录里有采集无法保存的内容。
          {c.kind === 'candidate' && ' 它报告的 done 还没有生效。'}
        </p>
      )}
      <ul className="paths">
        {(c.state === 'oversized' ? files.map((f) => `${f.path}（${bytes(f.size)}）`) : paths.map((p) => `${p.path}（${p.kind}）`))
          .slice(0, 12)
          .map((line) => (
            <li key={line}>{line}</li>
          ))}
        {files.length + paths.length > 12 && <li className="muted">……共 {files.length + paths.length} 项</li>}
      </ul>
      <div className="actions">
        {c.state === 'oversized' && <button onClick={() => decide('approve_new_files')}>放行</button>}
        <button onClick={() => decide('discard_uncaptured')}>{c.state === 'oversized' ? '不要这些文件' : '跳过这些内容'}</button>
        <button className="primary" onClick={proceed} title="把清单交给执行者处理">
          继续
        </button>
      </div>
    </div>
  );
}

function UndeliveredNote({ taskId }: { taskId: string }) {
  const count = useStore((s) => s.snapshot?.tasks.find((t) => t.id === taskId)?.undelivered_messages ?? 0);
  if (count === 0) return null;
  return <p className="muted">还有 {count} 条你发的消息没交给执行者，验收前要先决定怎么处理。</p>;
}

/** A turn outside any task changed files; nothing writes over them until the user decides (#14). */
function OutsideChangesCard({ role, capture }: { role: string; capture: Capture }) {
  const changed = capture.detail?.changed ?? [];
  const [viewing, setViewing] = useState(false);
  const [adopting, setAdopting] = useState(false);
  return (
    <div className="card warn">
      <p>
        {role} 在任务之外改了 {changed.length} 个文件。这些改动不属于任何任务，不会被验收或发布；你决定之前，不会开始下一个任务。
      </p>
      <ul className="paths">
        {changed.slice(0, 12).map((path) => (
          <li key={path}>{path}</li>
        ))}
        {changed.length > 12 && <li className="muted">……共 {changed.length} 个文件</li>}
      </ul>
      <div className="actions">
        <button onClick={() => setViewing(true)}>查看改动</button>
        <button onClick={() => run('discard_outside_changes', { capture_id: capture.id })} title="工作目录回到集成版本；采集记录仍保留这些改动">
          丢弃这些改动
        </button>
        <button className="primary" onClick={() => setAdopting(true)}>
          建成任务…
        </button>
      </div>
      {viewing && capture.commit_id && (
        <Modal title="任务之外的改动" onClose={() => setViewing(false)} wide>
          <DiffPool>
            <DiffView from={capture.base} to={capture.commit_id} />
          </DiffPool>
        </Modal>
      )}
      {adopting && <NewTask fromCapture={capture.id} onClose={() => setAdopting(false)} />}
    </div>
  );
}

function BlockedCard({ taskId, title, reason }: { taskId: string; title: string; reason: string }) {
  return (
    <div className="card">
      <p>
        「{title}」的执行者在等你：
      </p>
      <p className="quote">{reason}</p>
      <Reply taskId={taskId} />
    </div>
  );
}

/** The turn ended normally, the task did not: nothing moves it on but the user (#13). */
function StalledCard({ attention: a }: { attention: Extract<Attention, { kind: 'stalled' }> }) {
  return (
    <div className="card">
      <p>
        {a.role} 的 turn 已经结束，但「{a.title}」还没有完成：它没有报告完成，也没有提出问题。
      </p>
      {a.report_error && (
        <p className="muted failure">它调用 org_report 没有成功：{a.report_error}</p>
      )}
      <p className="muted">看看它最后说了什么，再告诉它接下来做什么。</p>
      <Reply taskId={a.task_id} placeholder={`发给 ${a.role}`}>
        <button onClick={() => selectTask(a.task_id)}>查看任务</button>
      </Reply>
    </div>
  );
}

function Reply({ taskId, placeholder = '回复', children }: { taskId: string; placeholder?: string; children?: React.ReactNode }) {
  const [reply, setReply] = useState('');
  return (
    <>
      <textarea value={reply} onChange={(e) => setReply(e.target.value)} rows={2} placeholder={placeholder} aria-label={placeholder} />
      <div className="actions">
        {children}
        <button
          className="primary"
          disabled={!reply.trim()}
          onClick={async () => {
            const sent = await run('send_message', { role: MAIN_ROLE, task_id: taskId, body: reply.trim() });
            if (sent !== undefined) setReply('');
          }}
        >
          发送
        </button>
      </div>
    </>
  );
}

function Workboard() {
  const roles = useStore((s) => s.snapshot?.roles ?? []);
  const tasks = useStore((s) => s.snapshot?.tasks ?? []);
  const [creating, setCreating] = useState(false);
  const [showClosed, setShowClosed] = useState(false);
  const active = tasks.filter((t) => ['executing', 'verifying', 'accepting'].includes(t.phase));
  const queued = tasks.filter((t) => t.phase === 'queued').sort((a, b) => (a.queue_pos ?? 0) - (b.queue_pos ?? 0));
  const closed = tasks.filter((t) => t.phase === 'done' || t.phase === 'abandoned').reverse();
  return (
    <section className="workboard">
      <h3>
        Workboard
        <button className="small" onClick={() => setCreating(true)}>
          + 新任务
        </button>
      </h3>
      {roles.map((role) => (
        <RoleLine key={role.name} role={role} tasks={tasks} />
      ))}
      {active.map((t) => (
        <TaskLine key={t.id} task={t} />
      ))}
      {queued.length > 0 && <h4>排队</h4>}
      {queued.map((t, i) => (
        <TaskLine key={t.id} task={t} queueIndex={i} queueLength={queued.length} />
      ))}
      {closed.length > 0 && (
        <button className="link" onClick={() => setShowClosed(!showClosed)}>
          {showClosed ? '收起' : `已关闭 ${closed.length} 个`}
        </button>
      )}
      {showClosed && closed.map((t) => <TaskLine key={t.id} task={t} />)}
      {creating && <NewTask onClose={() => setCreating(false)} />}
    </section>
  );
}

function RoleLine({ role, tasks }: { role: RoleView; tasks: Task[] }) {
  const now = useNow(role.unfinished?.state === 'running');
  const task = tasks.find((t) => t.id === role.task_id);
  let activity = '空闲';
  if (role.unfinished?.state === 'running' && role.unfinished.started_at) activity = `正在跑 turn · ${elapsed(role.unfinished.started_at, now)}`;
  else if (role.unfinished) activity = role.unfinished.state === 'unknown' ? '上一个 turn 状态未知' : '即将开始 turn';
  else if (role.hold) activity = '停下，等你决定';
  else if (role.outside) activity = '任务之外有改动，等你决定';
  else if (role.stalled) activity = 'turn 已结束，任务还没完成，等你发消息';
  else if (role.workspace?.state === 'materializing') activity = '正在准备工作目录';
  else if (task) activity = PHASE_LABEL[task.phase];
  return (
    <div className="role-line">
      <span className="role-name">{role.name}</span>
      <span className="muted">{activity}</span>
    </div>
  );
}

function TaskLine({ task, queueIndex, queueLength }: { task: Task; queueIndex?: number; queueLength?: number }) {
  const selected = useStore((s) => s.selectedTask === task.id);
  const move = (to: number) => run('move_in_queue', { task_id: task.id, to_index: to });
  return (
    <div className={`task-line ${selected ? 'selected' : ''}`} onClick={() => selectTask(task.id)} role="button">
      <span className={`phase ${task.phase}`}>{PHASE_LABEL[task.phase]}</span>
      <span className="task-title">{task.title}</span>
      {task.paused && <span className="badge">已暂停</span>}
      {task.attempt_seq && task.attempt_seq > 1 && <span className="muted">第 {task.attempt_seq} 轮</span>}
      {queueIndex !== undefined && queueLength !== undefined && (
        <span className="reorder" onClick={(e) => e.stopPropagation()}>
          <button className="ghost" disabled={queueIndex === 0} onClick={() => move(queueIndex - 1)} aria-label="上移">
            ↑
          </button>
          <button className="ghost" disabled={queueIndex === queueLength - 1} onClick={() => move(queueIndex + 1)} aria-label="下移">
            ↓
          </button>
        </span>
      )}
    </div>
  );
}

/** A new task; with `fromCapture`, made from changes outside any task and starting from them (#14). */
function NewTask({ onClose, fromCapture }: { onClose: () => void; fromCapture?: string }) {
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [criteria, setCriteria] = useState('');
  const create = async () => {
    const created = fromCapture
      ? await run<{ id: string }>('adopt_outside_changes', { capture_id: fromCapture, title, body, criteria })
      : await run<{ id: string }>('create_task', { title, body, criteria, executor: MAIN_ROLE });
    if (!created) return;
    onClose();
    // An open task panel would cover the thread where the new task's work shows (#13); the
    // earlier task stays a click away in the Workboard.
    selectTask(null);
    toast(`「${title}」已交给 ${MAIN_ROLE}`);
  };
  return (
    <Modal title={fromCapture ? '用这些改动建任务' : '新任务'} onClose={onClose}>
      {fromCapture && <p className="muted">任务从这些改动开始：执行者在它们的基础上继续，做完后照常验证、验收。</p>}
      <label>
        标题
        <input value={title} onChange={(e) => setTitle(e.target.value)} autoFocus />
      </label>
      <label>
        你的原话
        <textarea value={body} onChange={(e) => setBody(e.target.value)} rows={6} placeholder="要做什么、为什么、有什么要注意的" />
      </label>
      <label>
        完成条件
        <textarea value={criteria} onChange={(e) => setCriteria(e.target.value)} rows={3} placeholder="怎样算完成，最好能被检查" />
      </label>
      <div className="actions">
        <button onClick={onClose}>取消</button>
        <button className="primary" disabled={!title.trim()} onClick={create}>
          交给 Malkuth
        </button>
      </div>
    </Modal>
  );
}
