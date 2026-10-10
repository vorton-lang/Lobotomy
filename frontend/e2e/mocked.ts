// What a mocked backend answers to `host`: one open project, whose id pushes about it carry
// (frontend.md §3 rule 8).
import type { HostSnapshot } from '../src/api/types';

export const PROJECT = 'project';

export const hostSnapshot = (): HostSnapshot => ({
  projects: [{ id: PROJECT, name: 'project', data_dir: '/data/project', repo_path: '/project', state: 'running', registered_at: 1, error: null, attention: [], busy: false }],
  attention: [],
  harnesses: [],
});
