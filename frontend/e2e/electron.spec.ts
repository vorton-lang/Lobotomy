// The Electron shell (frontend.md §1), with a host directory of its own so the user's is never
// touched: it starts the backend, one for all projects, and the window connects to it; without a
// project it asks for a repository.
//
// That the backend outlives the app is not checked here: on Windows the backend inherits the
// app's stdout pipe, and Playwright waits for that pipe to close before it lets the app go.

import { _electron as electron, expect, test } from '@playwright/test';
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// Electron's single-instance lock uses a shared profile, even with separate Lobotomy hosts.
// Keep shell tests together in one worker while the isolated browser scenarios run in parallel.
test.describe.configure({ mode: 'default' });

const app = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const backendExe = path.join(app, '..', 'backend', 'target', 'debug', process.platform === 'win32' ? 'lobotomyd.exe' : 'lobotomyd');

/** A host directory of the test's own, removed after the test. */
const hosts: string[] = [];
function tempHost() {
  const host = fs.mkdtempSync(path.join(os.tmpdir(), 'lobotomy-electron-'));
  hosts.push(host);
  return host;
}
test.afterEach(() => {
  // The retries wait for Windows to let go of a stopped backend's files.
  for (const host of hosts.splice(0)) fs.rmSync(host, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
});

async function launch(host: string) {
  return electron.launch({ args: [app], cwd: app, env: { ...process.env, LOBOTOMY_HOST_DIR: host } });
}

test('the app starts the backend and, without a project, asks for a repository', async () => {
  const host = tempHost();
  const electronApp = await launch(host);
  const window = await electronApp.firstWindow();
  // The onboarding screen, served by the running backend.
  await expect(window.getByRole('button', { name: '选择仓库文件夹' })).toBeEnabled();
  const info = JSON.parse(fs.readFileSync(path.join(host, 'backend.json'), 'utf8')) as { pid: number };
  expect(() => process.kill(info.pid, 0)).not.toThrow();
  process.kill(info.pid);
  await electronApp.close();
});

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

test('after the backend restarts on another port the window reconnects', async () => {
  const host = tempHost();
  const repo = path.join(host, 'repo');
  fs.mkdirSync(repo);
  const git = (...args: string[]) => execFileSync('git', ['-C', repo, ...args], { stdio: 'pipe' });
  git('init', '--quiet', '--initial-branch', 'main');
  git('config', 'user.name', 'Test');
  git('config', 'user.email', 'test@example.com');
  git('commit', '--quiet', '--allow-empty', '-m', 'initial');
  // Started here, not by the app, so that the test can stop and restart it.
  const info = path.join(host, 'backend.json');
  const backend = async () => {
    fs.rmSync(info, { force: true });
    const child = spawn(backendExe, ['--host-dir', host, '--repo', repo], { stdio: 'ignore' });
    await expect.poll(() => fs.existsSync(info)).toBe(true);
    return { child, port: (JSON.parse(fs.readFileSync(info, 'utf8')) as { port: number }).port };
  };
  const first = await backend();
  const electronApp = await launch(host);
  const window = await electronApp.firstWindow();
  await expect(window.locator('.status.open')).toBeVisible();

  first.child.kill();
  await expect(window.locator('.status.closed, .status.connecting')).toBeVisible();
  const second = await backend();
  expect(second.port).not.toBe(first.port);
  await expect(window.locator('.status.open')).toBeVisible();
  second.child.kill();
  await electronApp.close();
});
