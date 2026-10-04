// Starts a backend for the end-to-end tests: a fresh host directory, a fresh project connected to
// a fresh git repository, and the fake Codex. The backend is built beforehand with
// `cargo build -p lobotomyd`.

import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const backend = path.resolve(here, '..', '..', 'backend');
const exe = path.join(backend, 'target', 'debug', process.platform === 'win32' ? 'lobotomyd.exe' : 'lobotomyd');
const fakeCodex = path.join(backend, 'crates', 'lobotomyd', 'tests', 'fixtures', 'fake-codex.mjs');
export const infoFile = path.join(os.tmpdir(), 'lobotomy-e2e.json');

const git = (cwd: string, ...args: string[]) => execFileSync('git', ['-C', cwd, ...args], { stdio: 'pipe' });

export default async function setup() {
  if (!fs.existsSync(exe)) throw new Error(`build the backend first: cargo build -p lobotomyd (${exe})`);
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'lobotomy-e2e-'));
  const repo = path.join(root, 'repo');
  fs.mkdirSync(repo);
  git(repo, 'init', '--quiet', '--initial-branch', 'main');
  git(repo, 'config', 'user.name', 'Test User');
  git(repo, 'config', 'user.email', 'user@example.com');
  fs.writeFileSync(path.join(repo, 'README.md'), '# project\n');
  git(repo, 'add', '.');
  git(repo, 'commit', '--quiet', '-m', 'initial');

  const data = path.join(root, 'project');
  const host = path.join(root, 'host');
  const log = fs.openSync(path.join(root, 'backend.log'), 'a');
  const child = spawn(exe, ['--data-dir', data, '--host-dir', host, '--repo', repo], {
    env: { ...process.env, LOBOTOMY_CODEX: JSON.stringify(['node', fakeCodex]) },
    stdio: ['ignore', log, log],
  });
  for (let i = 0; i < 100 && !fs.existsSync(path.join(data, 'backend.json')); i++) await new Promise((r) => setTimeout(r, 100));
  const { port } = JSON.parse(fs.readFileSync(path.join(data, 'backend.json'), 'utf8'));
  const token = fs.readFileSync(path.join(host, 'gui-token'), 'utf8').trim();
  fs.writeFileSync(infoFile, JSON.stringify({ port, token, repo, root }));

  return async () => {
    child.kill();
    await new Promise((resolve) => (child.exitCode !== null ? resolve(null) : child.once('exit', resolve)));
    // The retries wait for Windows to let go of the backend's files.
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
  };
}
