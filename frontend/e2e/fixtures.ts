// Each test gets a backend of its own (#16): a fresh host directory with one project connected to a
// fresh git repository, and Angela on the fake Codex, whose synthetic turns make long threads
// (fake-bench.mjs). Starting one takes a fraction of a second, and no test depends on what an earlier
// one left. The backend is built beforehand with
// `cargo build -p lobotomyd`; `LOBOTOMYD` names another build, as for the app, such as one in its
// own target directory while the app runs the usual one.

import { test as base, expect, type Page } from '@playwright/test';
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { devOrigin } from '../dev-server.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const backendDir = path.resolve(here, '..', '..', 'backend');
const exe =
  process.env.LOBOTOMYD ??
  path.join(backendDir, 'target', 'debug', process.platform === 'win32' ? 'lobotomyd.exe' : 'lobotomyd');
const fakeCodex = path.join(backendDir, 'crates', 'lobotomyd', 'tests', 'fixtures', 'fake-codex.mjs');
const fakeClaude = path.join(backendDir, 'crates', 'lobotomyd', 'tests', 'fixtures', 'fake-claude.mjs');

export interface Backend {
  port: number;
  token: string;
  /** The user's repository the project is connected to. */
  repo: string;
}

const git = (cwd: string, ...args: string[]) => execFileSync('git', ['-C', cwd, ...args], { stdio: 'pipe' });

async function startBackend(): Promise<Backend & { stop: (keepLog?: string) => Promise<void> }> {
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

  const host = path.join(root, 'host');
  const log = fs.openSync(path.join(root, 'backend.log'), 'a');
  const child = spawn(exe, ['--host-dir', host, '--repo', repo], {
    env: {
      ...process.env,
      LOBOTOMY_CODEX: JSON.stringify(['node', fakeCodex]),
      LOBOTOMY_CLAUDE: JSON.stringify(['node', fakeClaude]),
    },
    stdio: ['ignore', log, log],
  });
  const info = path.join(host, 'backend.json');
  for (let i = 0; i < 200 && !fs.existsSync(info) && child.exitCode === null; i++)
    await new Promise((r) => setTimeout(r, 50));
  if (!fs.existsSync(info)) {
    // Says whether it exited (a Windows status reads best in hex) or was still starting.
    const { exitCode: code, signalCode: signal } = child;
    const state =
      code !== null ? `exited with ${code} (0x${(code >>> 0).toString(16)})` : (signal ?? 'still starting after 10 s');
    child.kill();
    fs.closeSync(log);
    throw new Error(`the backend did not start, ${state}: ${fs.readFileSync(path.join(root, 'backend.log'), 'utf8')}`);
  }
  const { port } = JSON.parse(fs.readFileSync(info, 'utf8')) as { port: number };
  const token = fs.readFileSync(path.join(host, 'gui-token'), 'utf8').trim();
  await angelaOnCodex(port, token);

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

/** Puts Angela on the fake Codex through the GUI service, as the settings do. */
async function angelaOnCodex(port: number, token: string) {
  const socket = new WebSocket(`ws://127.0.0.1:${port}/gui?token=${encodeURIComponent(token)}`);
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  const reply = new Promise<{ error?: { message: string } }>((resolve) => {
    socket.onmessage = (event) => {
      const message = JSON.parse(String(event.data));
      if (message.id === 1) resolve(message);
    };
  });
  const args = { request_id: 'e2e-angela', role: 'Angela', harness: 'codex' };
  const params = { project: 'angela', name: 'set_role_harness', args };
  socket.send(JSON.stringify({ id: 1, method: 'command', params }));
  const { error } = await reply;
  socket.close();
  if (error) throw new Error(`Angela stays on Claude: ${error.message}`);
}

export const test = base.extend<{ backend: Backend }>({
  // Playwright inspects the destructured fixture dependencies, even when there are none.
  // oxlint-disable-next-line no-empty-pattern
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
  await page.goto(`${devOrigin}/?backend=${encodeURIComponent(url)}&token=${encodeURIComponent(backend.token)}`);
  await expect(page.getByText('已连接')).toBeVisible();
}

/** Shows 「全部」, Angela's conversation: the window opens on the backend's one project. */
export async function showAngela(page: Page) {
  await page.getByRole('navigation', { name: '项目' }).getByRole('button', { name: /^全部/ }).click();
  await expect(page.getByRole('region', { name: 'Angela' })).toBeVisible();
}
