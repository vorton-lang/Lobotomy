// What a mocked backend answers: `host` with one open project, whose id pushes about it carry
// (frontend.md §3 rule 8), and builders of the project's roles, tasks and task details.
import type { HostSnapshot, RoleView, TaskDetail, TaskView } from '../src/api/types';

export const PROJECT = 'project';

export const hostSnapshot = (): HostSnapshot => ({
  projects: [{ id: PROJECT, name: 'project', data_dir: '/data/project', repo_path: '/project', state: 'running', registered_at: 1, error: null, attention: [], busy: false }],
  attention: [],
  harnesses: [],
});

export const role = (taskId: string | null): RoleView => ({
  name: 'Malkuth', kind: 'worker', harness: 'codex', model: null, slot: 'worker', task_id: taskId, unfinished: null,
  last_turn: null, hold: null, stalled: null, outside: null, workspace: null, queued_messages: 0,
});

export const task = (id: string, title: string, phase: TaskView['phase'], undelivered = 0): TaskView => ({
  id, title, body: '', executor: 'Malkuth', phase, paused: false, blocked_reason: null, criteria_version: 1,
  queue_pos: null, revision: 1, created_at: 1, closed_at: phase === 'done' ? 9 : null, origin_capture: null,
  attempt_seq: phase === 'queued' ? null : 1, attempt_open: phase === 'executing', verification: null,
  undelivered_messages: undelivered,
});

/** A task whose first round's candidate passed verification. */
export const detail = (view: TaskView, undelivered: TaskDetail['undelivered_messages'] = []): TaskDetail => {
  const { id, title, body, executor, phase, paused, blocked_reason, criteria_version, queue_pos, revision, created_at, closed_at, origin_capture } = view;
  return {
    task: { id, title, body, executor, phase, paused, blocked_reason, criteria_version, queue_pos, revision, created_at, closed_at, origin_capture },
    criteria: [{ version: 1, text: 'work.txt 存在', created_by: 'user', created_at: 1 }],
    attempts: [{
      id: `a-${view.id}`, task_id: view.id, seq: 1, started_at: 1, ended_at: 2, end_reason: 'candidate', code_start: null,
      done_turn_id: `t-${view.id}`, done_summary: '完成\n\n写了 work.txt', trial: null, candidate_id: `c-${view.id}`, conflicts: [],
    }],
    captures: [{
      id: `c-${view.id}`, turn_id: `t-${view.id}`, role: 'Malkuth', task_id: view.id, attempt_id: `a-${view.id}`, kind: 'candidate',
      base: 'base0000', state: 'pinned', commit_id: '1111111111', detail: null, created_at: 2, outside: null,
    }],
    verifications: [{
      id: `v-${view.id}`, task_id: view.id, attempt_id: `a-${view.id}`, capture_id: `c-${view.id}`, base: 'base0000', config_version: 1,
      commit_id: '2222222222', conflicts: [], state: 'passed', created_at: 3, finished_at: 4,
    }],
    checks: [], decisions: [], publications: [], undelivered_messages: undelivered,
  };
};
