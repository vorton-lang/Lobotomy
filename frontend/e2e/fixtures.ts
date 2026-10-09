// Each test gets a backend of its own (#16): a fresh host directory, a fresh project connected to a
// fresh git repository, and the fake Codex. Starting one takes a fraction of a second, and no test
// depends on what an earlier one left. The backend is built beforehand with
// `cargo build -p lobotomyd`.

import { test as base, expect, type Page } from '@playwright/test';
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const backendDir = path.resolve(here, '..', '..', 'backend');
const exe = path.join(backendDir, 'target', 'debug', process.platform === 'win32' ? 'lobotomyd.exe' : 'lobotomyd');
const fakeCodex = path.join(backendDir, 'crates', 'lobotomyd', 'tests', 'fixtures', 'fake-codex.mjs');
const fakeClaude = path.join(backendDir, 'crates', 'lobotomyd', 'tests', 'fixtures', 'fake-claude.mjs');

export interface Backend {
  port: number;
  token: string;
  /** The user's repository the project is connected to. */
  repo: string;
}

const git = (cwd: string, ...args: string[]) => execFileSync('git', ['-C', cwd, ...args], { stdio: 'pipe' });

async function startBackend(): Promise<Backend & { stop: () => Promise<void> }> {
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
    env: { ...process.env, LOBOTOMY_CODEX: JSON.stringify(['node', fakeCodex]), LOBOTOMY_CLAUDE: JSON.stringify(['node', fakeClaude]) },
    stdio: ['ignore', log, log],
  });
  const info = path.join(data, 'backend.json');
  for (let i = 0; i < 200 && !fs.existsSync(info) && child.exitCode === null; i++) await new Promise((r) => setTimeout(r, 50));
  if (!fs.existsSync(info)) throw new Error(`the backend did not start: ${fs.readFileSync(path.join(root, 'backend.log'), 'utf8')}`);
  const { port } = JSON.parse(fs.readFileSync(info, 'utf8')) as { port: number };
  const token = fs.readFileSync(path.join(host, 'gui-token'), 'utf8').trim();

  /** Stops the backend and removes its directories; its log goes to `keepLog` first, if given. */
  const stop = async (keepLog?: string) => {
    child.kill();
    await new Promise((resolve) => (child.exitCode !== null ? resolve(null) : child.once('exit', resolve)));
    fs.closeSync(log);
    if (keepLog) fs.copyFileSync(path.join(root, 'backend.log'), keepLog);
    // The retries wait for Windows to let go of the backend's files.
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
  };
  return { port, token, repo, stop };
}

export const test = base.extend<{ backend: Backend }>({
  // eslint-disable-next-line no-empty-pattern
  backend: async ({}, use, testInfo) => {
    const backend = await startBackend();
    try {
      await use(backend);
    } finally {
      // A failed test keeps the backend's log next to its trace.
      const failed = testInfo.status !== testInfo.expectedStatus;
      await backend.stop(failed ? testInfo.outputPath('backend.log') : undefined);
    }
  },
});

export { expect };

/** Opens the GUI on the test's backend and waits until it is connected. */
export async function open(page: Page, backend: Backend) {
  const url = `ws://127.0.0.1:${backend.port}/gui`;
  await page.goto(`http://127.0.0.1:5173/?backend=${encodeURIComponent(url)}&token=${encodeURIComponent(backend.token)}`);
  await expect(page.getByText('已连接')).toBeVisible();
}

/** Creates a task for Malkuth, the executor of the test project, through the "+ 新任务" dialog. */
export async function createTask(page: Page, title: string, body: string, { twice = false, criteria = 'work.txt 存在' } = {}) {
  await page.getByRole('button', { name: '+ 新任务' }).click();
  const dialog = page.getByRole('dialog', { name: '新任务' });
  await dialog.getByLabel('标题').fill(title);
  await dialog.getByLabel('你的原话').fill(body);
  await dialog.getByLabel('完成条件').fill(criteria);
  const submit = dialog.getByRole('button', { name: '交给 Malkuth' });
  if (twice) await submit.dblclick();
  else await submit.click();
}

/** The task panel shows a task over the thread; it is a region named by the task's title. */
export const taskPanel = (page: Page, title: string) => page.getByRole('region', { name: title });
