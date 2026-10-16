// Compile-only regressions: callers choose a method, not an arbitrary response type.
import type { Connection } from './connection';
import type { Created, SearchResult, Snapshot } from './generated';
import { call, run } from '../store';

export function checkCalls(connection: Connection) {
  const search: Promise<SearchResult> = connection.call('search', { role: 'Malkuth', query: 'text' });
  const snapshot: Promise<Snapshot> = call('snapshot');
  void [search, snapshot];
  connection.call('thread', { role: 'Malkuth', before: null });
  connection.command({ name: 'interrupt', args: { turn_id: 'turn' } });
  const created: Promise<Created | undefined> = run('create_task', {
    title: 'task',
    body: '',
    criteria: '',
    executor: 'Malkuth',
  });
  void created;
  // @ts-expect-error unknown method
  call('snapshop');
  // @ts-expect-error a search needs its query
  call('search', { role: 'Malkuth' });
  // @ts-expect-error a caller cannot choose the response type
  connection.call<Snapshot>('search', { role: 'Malkuth', query: '' });
  // @ts-expect-error wrong command arguments
  connection.command({ name: 'interrupt', args: { task_id: 'task' } });
  // @ts-expect-error command output is fixed by Rust's Command::Output
  const wrong: Promise<Snapshot> = run('create_task', { title: '', body: '', criteria: '', executor: '' });
  void wrong;
  // @ts-expect-error creating a task needs an executor
  run('create_task', { title: '', body: '', criteria: '' });
}
