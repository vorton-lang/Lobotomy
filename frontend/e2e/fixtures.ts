// Each test gets a backend of its own (#16): a fresh host directory with one project connected to a
// fresh git repository, and the fake Codex. Starting one takes a fraction of a second, and no test
// depends on what an earlier one left. The backend is built beforehand with
// `cargo build -p lobotomyd`; `LOBOTOMYD` names another build, as for the app, such as one in its
// own target directory while the app runs the usual one.

import { test as base, expect, type Page, type TestInfo } from '@playwright/test';
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

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

async function startBackend(testInfo: TestInfo): Promise<Backend & { stop: (keepLog?: string) => Promise<void> }> {
  const diagnostic = process.env.LOBOTOMY_STARTUP_DIAGNOSTICS !== undefined;
  const started = performance.now();
  const events: object[] = [];
  const record = (event: string, data: object = {}) => {
    if (diagnostic) events.push({ event, elapsedMs: Math.round(performance.now() - started), ...data });
  };
  const collect = (operation: string, action: () => void) => {
    try {
      action();
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code ?? 'unknown';
      record('diagnostic-error', { operation, code });
      // No raw exception text: it can contain paths or subprocess output.
      console.warn(`CI diagnostic collection failed: ${operation} (${code})`);
    }
  };
  record('prepare');
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
  record('spawn-request');
  const child = spawn(exe, ['--host-dir', host, '--repo', repo], {
    env: {
      ...process.env,
      LOBOTOMY_TEST_CHECK_GATE: path.join(root, 'check-gate'),
      LOBOTOMY_CODEX: JSON.stringify(['node', fakeCodex]),
      LOBOTOMY_CLAUDE: JSON.stringify(['node', fakeClaude]),
    },
    stdio: ['ignore', log, log],
  });
  if (diagnostic) {
    child.once('spawn', () => record('spawned', { pid: child.pid }));
    child.once('error', (error: NodeJS.ErrnoException) => {
      record('spawn-error', { code: error.code });
      snapshot('spawn-error');
      // Preserve the unhandled spawn error that the original fixture would raise.
      throw error;
    });
    child.once('exit', (code, signal) => record('exit', { code, signal }));
    child.once('close', (code, signal) => record('close', { code, signal }));
  }
  const snapshot = (event: string) => {
    if (!diagnostic) return;
    collect('process-state', () => {
      record(event, {
        pid: child.pid,
        exitCode: child.exitCode,
        exitCodeHex: child.exitCode === null ? null : `0x${(child.exitCode >>> 0).toString(16)}`,
        signalCode: child.signalCode,
        hostExists: fs.existsSync(host),
        infoExists: fs.existsSync(path.join(host, 'backend.json')),
        tokenExists: fs.existsSync(path.join(host, 'gui-token')),
        logBytes: fs.statSync(path.join(root, 'backend.log')).size,
      });
    });
    collect('startup-phases', () => {
      // Only our fixed phase records are added to successful-run artifacts, not the raw log.
      const phases = fs
        .readFileSync(path.join(root, 'backend.log'), 'utf8')
        .split(/\r?\n/)
        .filter((line) => /^startup pid=\d+ elapsed_ms=\d+ phase=[a-z-]+$/.test(line));
      fs.writeFileSync(testInfo.outputPath('startup-phases.log'), phases.join('\n'));
    });
    if (process.platform === 'win32' && event === 'not-ready') {
      collect('windows-snapshot', () => {
        execFileSync(
          'pwsh',
          [
            '-NoProfile',
            '-File',
            path.join(here, '..', 'scripts', 'ci-windows-snapshot.ps1'),
            '-OutputFile',
            testInfo.outputPath('startup-windows.json'),
          ],
          { timeout: 15000, stdio: 'ignore' },
        );
      });
    }
    collect('startup-json', () =>
      fs.writeFileSync(testInfo.outputPath('startup-diagnostics.json'), JSON.stringify(events, null, 2)),
    );
  };
  const info = path.join(host, 'backend.json');
  for (let i = 0; i < 200 && !fs.existsSync(info) && child.exitCode === null; i++)
    await new Promise((r) => setTimeout(r, 50));
  // Freeze the original decision before collecting diagnostics; collection cannot turn a failure green.
  const ready = fs.existsSync(info);
  if (!ready) {
    const error = new Error(`the backend did not start: ${fs.readFileSync(path.join(root, 'backend.log'), 'utf8')}`);
    snapshot('not-ready');
    throw error;
  }
  const { port } = JSON.parse(fs.readFileSync(info, 'utf8')) as { port: number };
  const token = fs.readFileSync(path.join(host, 'gui-token'), 'utf8').trim();
  snapshot('ready');

  /** Stops the backend and removes its directories; its log goes to `keepLog` first, if given. */
  const stop = async (keepLog?: string) => {
    record('stop-request');
    child.kill();
    await new Promise((resolve) => (child.exitCode !== null ? resolve(null) : child.once('exit', resolve)));
    snapshot('stopped');
    fs.closeSync(log);
    if (keepLog) fs.copyFileSync(path.join(root, 'backend.log'), keepLog);
    // The retries wait for Windows to let go of the backend's files.
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
  };
  return { port, token, repo, stop };
}

export const test = base.extend<{ backend: Backend }>({
  // Playwright inspects the destructured fixture dependencies, even when there are none.
  // oxlint-disable-next-line no-empty-pattern
  backend: async ({}, use, testInfo) => {
    const backend = await startBackend(testInfo);
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
  await page.goto(
    `http://127.0.0.1:5173/?backend=${encodeURIComponent(url)}&token=${encodeURIComponent(backend.token)}`,
  );
  await expect(page.getByText('已连接')).toBeVisible();
}

/** Creates a task for Malkuth, the executor of the test project, through the "+ 新任务" dialog. */
export async function createTask(
  page: Page,
  title: string,
  body: string,
  { twice = false, criteria = 'work.txt 存在' } = {},
) {
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
