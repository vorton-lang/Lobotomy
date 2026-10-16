// The Electron shell (frontend.md §1), with a host directory of its own so the user's is never
// touched: it starts the backend, one for all projects, and the window connects to it; without a
// project it asks for a repository.
//
// That the backend outlives the app is not checked here: on Windows the backend inherits the
// app's stdout pipe, and Playwright waits for that pipe to close before it lets the app go.

import { _electron as electron, expect, test } from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// Electron's single-instance lock uses a shared profile, even with separate Lobotomy hosts.
// Keep shell tests together in one worker while the isolated browser scenarios run in parallel.
test.describe.configure({ mode: 'default' });

const app = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/** A host directory of the test's own, removed after the test. */
const hosts: string[] = [];
function tempHost() {
  const host = fs.mkdtempSync(path.join(os.tmpdir(), 'lobotomy-electron-'));
  hosts.push(host);
  return host;
}
test.afterEach(() => {
  // The retries wait for Windows to let go of a stopped backend's files.
  for (const host of hosts.splice(0))
    fs.rmSync(host, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
});

async function launch(host: string) {
  return electron.launch({ args: [app], cwd: app, env: { ...process.env, LOBOTOMY_HOST_DIR: host } });
}

test('quitting the app stops the organization', async () => {
  const host = tempHost();
  const electronApp = await launch(host);
  const window = await electronApp.firstWindow();
  await expect(window.getByRole('button', { name: '选择仓库文件夹' })).toBeEnabled();
  const info = JSON.parse(fs.readFileSync(path.join(host, 'backend.json'), 'utf8')) as { pid: number };
  const exited = new Promise((resolve) => electronApp.process().once('exit', resolve));
  await electronApp.evaluate(() => (globalThis as unknown as { lobotomyQuit: () => Promise<void> }).lobotomyQuit());
  await exited;
  const alive = () => {
    try {
      process.kill(info.pid, 0);
      return true;
    } catch {
      return false;
    }
  };
  await expect.poll(alive).toBe(false);
  expect(fs.existsSync(path.join(host, 'backend.json'))).toBe(false);
});
