// The Electron shell (frontend.md §1), with a host directory of its own so the user's is never
// touched: without a project it asks for a repository; with one it starts the backend and the
// window connects to it.
//
// That the backend outlives the app is not checked here: on Windows the backend inherits the
// app's stdout pipe, and Playwright waits for that pipe to close before it lets the app go.

import { _electron as electron, expect, test } from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const app = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

async function launch(host: string) {
  return electron.launch({ args: [app], cwd: app, env: { ...process.env, LOBOTOMY_HOST_DIR: host } });
}

test('without a project the app asks for a repository', async () => {
  const host = fs.mkdtempSync(path.join(os.tmpdir(), 'lobotomy-electron-'));
  const electronApp = await launch(host);
  const window = await electronApp.firstWindow();
  await expect(window.getByRole('button', { name: '选择仓库文件夹' })).toBeVisible();
  await electronApp.close();
});

test('quitting the app stops the organization', async () => {
  const host = fs.mkdtempSync(path.join(os.tmpdir(), 'lobotomy-electron-'));
  const project = path.join(host, 'projects', 'p');
  fs.mkdirSync(project, { recursive: true });
  fs.writeFileSync(path.join(host, 'gui.json'), JSON.stringify({ project }));
  const electronApp = await launch(host);
  const window = await electronApp.firstWindow();
  await expect(window.getByRole('button', { name: '选择仓库文件夹' })).toBeVisible();
  const info = JSON.parse(fs.readFileSync(path.join(project, 'backend.json'), 'utf8')) as { pid: number };
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
  expect(fs.existsSync(path.join(project, 'backend.json'))).toBe(false);
});

test('with a project the app starts its backend and connects', async () => {
  const host = fs.mkdtempSync(path.join(os.tmpdir(), 'lobotomy-electron-'));
  const project = path.join(host, 'projects', 'p');
  fs.mkdirSync(project, { recursive: true });
  fs.writeFileSync(path.join(host, 'gui.json'), JSON.stringify({ project }));
  const electronApp = await launch(host);
  const window = await electronApp.firstWindow();
  // Not connected to a repository yet: the onboarding screen, served by the running backend.
  await expect(window.getByRole('button', { name: '选择仓库文件夹' })).toBeVisible();
  const info = JSON.parse(fs.readFileSync(path.join(project, 'backend.json'), 'utf8')) as { pid: number };
  expect(() => process.kill(info.pid, 0)).not.toThrow();
  process.kill(info.pid);
  await electronApp.close();
});
