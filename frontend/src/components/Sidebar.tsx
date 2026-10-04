// The right column (frontend.md §2): what waits for the user, then the Workboard.

import { useState } from 'react';
import type { Attention, Hold, RoleView, Task } from '../api/types';
import { bytes, clock, elapsed, PHASE_LABEL } from '../format';
import { act, MAIN_ROLE, run, selectTask, useStore } from '../store';
import { Modal, useNow } from './common';

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
    case 'accept':
      return (
        <div className="card">
          <p>
            「{a.title}」通过了验证，等你验收。
          </p>
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
  const taskId = roleView?.task_id;
  const proceed = () => run('continue', { role });
  const newSession = () => run('new_native_session', { role });
  const abandon = taskId ? () => run('abandon', { task_id: taskId, reason: '用户在异常后放弃' }) : undefined;
  if (hold.kind === 'abnormal') {
    const why = hold.turn.outcome === 'interrupted' ? '被中断' : hold.turn.failure?.kind === 'quota' ? '因额度不足而失败' : '失败';
    return (
      <div className="card warn">
        <p>
          {role} 的上一个 turn {why}，它停下等你决定。
        </p>
        {hold.turn.failure && <p className="muted">{hold.turn.failure.message}</p>}
        <div className="actions">
          <button className="primary" onClick={proceed}>
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

function BlockedCard({ taskId, title, reason }: { taskId: string; title: string; reason: string }) {
  const [reply, setReply] = useState('');
  return (
    <div className="card">
      <p>
        「{title}」的执行者在等你：
      </p>
      <p className="quote">{reason}</p>
      <textarea value={reply} onChange={(e) => setReply(e.target.value)} rows={2} placeholder="回复" aria-label="回复" />
      <div className="actions">
        <button
          className="primary"
          disabled={!reply.trim()}
          onClick={async () => {
            const sent = await run('send_message', { role: MAIN_ROLE, task_id: taskId, body: reply.trim() });
            if (sent !== undefined) setReply('');
          }}
        >
          回复
        </button>
      </div>
    </div>
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

function NewTask({ onClose }: { onClose: () => void }) {
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [criteria, setCriteria] = useState('');
  const create = async () => {
    const created = await run<{ id: string }>('create_task', { title, body, criteria, executor: MAIN_ROLE });
    if (created) onClose();
  };
  return (
    <Modal title="新任务" onClose={onClose}>
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
