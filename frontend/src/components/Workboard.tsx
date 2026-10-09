// The Workboard (frontend.md §2): each role and what it does, the tasks in work, the queue, and
// the closed tasks on demand.

import { useState } from 'react';
import type { RoleView, TaskView } from '../api/types';
import { PHASE_LABEL, roleActivity } from '../format';
import { run, selectTask, useMainRole, useStore } from '../store';
import { useNow } from './common';
import { NewTask } from './NewTask';

export function Workboard() {
  const roles = useStore((s) => s.snapshot?.roles ?? []);
  const tasks = useStore((s) => s.snapshot?.tasks ?? []);
  const executor = useMainRole();
  const [creating, setCreating] = useState(false);
  const [showClosed, setShowClosed] = useState(false);
  const active = tasks.filter((t) => ['executing', 'verifying', 'accepting'].includes(t.phase));
  const queued = tasks.filter((t) => t.phase === 'queued').sort((a, b) => (a.queue_pos ?? 0) - (b.queue_pos ?? 0));
  const closed = tasks.filter((t) => t.phase === 'done' || t.phase === 'abandoned').reverse();
  return (
    <section className="workboard" aria-label="Workboard">
      <h3>
        Workboard
        <button className="small" onClick={() => setCreating(true)} disabled={!executor}>
          + 新任务
        </button>
      </h3>
      {roles.map((role) => (
        <RoleLine key={role.name} role={role} task={tasks.find((t) => t.id === role.task_id)} />
      ))}
      {active.map((t) => (
        <TaskLine key={t.id} task={t} />
      ))}
      {queued.length > 0 && <h4>排队</h4>}
      {queued.map((t, i) => (
        <TaskLine key={t.id} task={t} queue={{ index: i, length: queued.length }} />
      ))}
      {closed.length > 0 && (
        <button className="link" onClick={() => setShowClosed(!showClosed)} aria-expanded={showClosed}>
          {showClosed ? '收起' : `已关闭 ${closed.length} 个`}
        </button>
      )}
      {showClosed && closed.map((t) => <TaskLine key={t.id} task={t} />)}
      {creating && executor && <NewTask executor={executor.name} onClose={() => setCreating(false)} />}
    </section>
  );
}

function RoleLine({ role, task }: { role: RoleView; task: TaskView | undefined }) {
  const now = useNow(role.unfinished?.state === 'running');
  return (
    <div className="role-line">
      <span className="role-name">{role.name}</span>
      <span className="muted">{roleActivity(role, task, now)}</span>
    </div>
  );
}

/** A task opens in the panel; a queued one also moves up and down its queue. */
function TaskLine({ task, queue }: { task: TaskView; queue?: { index: number; length: number } }) {
  const selected = useStore((s) => s.selectedTask === task.id);
  const move = (to: number) => run('move_in_queue', { task_id: task.id, to_index: to });
  return (
    <div className={`task-line ${selected ? 'selected' : ''}`}>
      <button className="task-open" onClick={() => selectTask(task.id)} aria-current={selected || undefined}>
        <span className={`badge phase ${task.phase}`}>{PHASE_LABEL[task.phase]}</span>
        <span className="task-title">{task.title}</span>
        {task.paused && <span className="badge">已暂停</span>}
        {task.attempt_seq && task.attempt_seq > 1 && <span className="muted">第 {task.attempt_seq} 轮</span>}
      </button>
      {queue && (
        <span className="reorder">
          <button className="ghost" disabled={queue.index === 0} onClick={() => move(queue.index - 1)} aria-label={`上移「${task.title}」`}>
            ↑
          </button>
          <button
            className="ghost"
            disabled={queue.index === queue.length - 1}
            onClick={() => move(queue.index + 1)}
            aria-label={`下移「${task.title}」`}
          >
            ↓
          </button>
        </span>
      )}
    </div>
  );
}
