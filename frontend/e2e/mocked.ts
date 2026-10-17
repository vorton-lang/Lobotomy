// What a mocked backend answers: `host` with one open project, whose id pushes about it carry
// (frontend.md §3 rule 8), and builders of the project's roles, tasks and task details.
import type { HostSnapshot, RoleView, TaskDetail, TaskView } from '../src/api/types';

export const PROJECT = 'project';

export const hostSnapshot = (): HostSnapshot => ({
  projects: [
    {
      id: PROJECT,
      name: 'project',
      data_dir: '/data/project',
      repo_path: '/project',
      state: 'running',
      registered_at: 1,
      error: null,
      attention: [],
      busy: false,
    },
  ],
  attention: [],
  harnesses: [],
  angela: { error: null, busy: false, attention: [] },
});

export const role = (taskId: string | null): RoleView => ({
  name: 'Malkuth',
  kind: 'worker',
  harness: 'codex',
  model: null,
  effort: null,
  remote: false,
  task_id: taskId,
  unfinished: null,
  last_turn: null,
  hold: null,
  stalled: null,
  queued_messages: 0,
});

export const task = (id: string, title: string, phase: TaskView['phase'], undelivered = 0): TaskView => ({
  id,
  title,
  body: '',
  executor: 'Malkuth',
  phase,
  paused: false,
  paused_by: null,
  blocked_reason: null,
  criteria_version: 1,
  queue_pos: null,
  revision: 1,
  created_at: 1,
  closed_at: phase === 'done' ? 9 : null,
  assignment: null,
  attempt_seq: phase === 'queued' ? null : 1,
  attempt_open: phase === 'executing',
  undelivered_messages: undelivered,
  reply_queued: undelivered > 0,
  worker: null,
});

/** A task whose first round handed in its candidate. */
export const detail = (view: TaskView, undelivered: TaskDetail['undelivered_messages'] = []): TaskDetail => {
  const {
    attempt_seq: _seq,
    attempt_open: _open,
    undelivered_messages: _undelivered,
    reply_queued: _reply,
    ...task
  } = view;
  return {
    task,
    criteria: [{ version: 1, text: 'work.txt 存在', created_by: 'user', created_at: 1 }],
    attempts: [
      {
        id: `a-${view.id}`,
        task_id: view.id,
        seq: 1,
        started_at: 1,
        ended_at: 2,
        end_reason: 'candidate',
        done_turn_id: `t-${view.id}`,
        done_summary: '完成\n\n写了 work.txt',
      },
    ],
    decisions: [],
    undelivered_messages: undelivered,
  };
};
