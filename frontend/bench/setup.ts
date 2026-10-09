// The performance baseline's backend (frontend.md §5): the release build with the fake Codex on a
// fixed port, so a restart keeps the GUI's address, and a thread of more than 2000 items made the
// way real ones are: turns of the fake Codex through the runner.

import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const backendDir = path.resolve(here, '..', '..', 'backend');
const exe = process.env.LOBOTOMYD ?? path.join(backendDir, 'target', 'release', process.platform === 'win32' ? 'lobotomyd.exe' : 'lobotomyd');
const fakeCodex = path.join(backendDir, 'crates', 'lobotomyd', 'tests', 'fixtures', 'fake-codex.mjs');
const fakeClaude = path.join(backendDir, 'crates', 'lobotomyd', 'tests', 'fixtures', 'fake-claude.mjs');
export const infoFile = path.join(os.tmpdir(), 'lobotomy-bench.json');

/** Turns that make the thread: four of 600 items, then a huge output and unclosed Markdown. */
export const TURNS = ['FAKE:many=600', 'FAKE:many=600', 'FAKE:many=600', 'FAKE:many=600', 'FAKE:huge FAKE:unclosed FAKE:marker=newest'];
export const FIRST_TASK = '基准数据起点';
/** Only the first turn's brief says this; every turn's divider names the task. */
export const START_MARKER = `任务：${FIRST_TASK}（第 1 轮执行）`;

export interface BenchInfo {
  port: number;
  token: string;
  root: string;
  backend: { exe: string; args: string[]; env: Record<string, string> };
  items: number;
}

const git = (cwd: string, ...args: string[]) => execFileSync('git', ['-C', cwd, ...args], { stdio: 'pipe' });

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as net.AddressInfo;
      server.close(() => resolve(port));
    });
    server.on('error', reject);
  });
}

export function startBackend(info: Pick<BenchInfo, 'backend' | 'root'>) {
  const log = fs.openSync(path.join(info.root, 'backend.log'), 'a');
  const child = spawn(info.backend.exe, info.backend.args, { env: { ...process.env, ...info.backend.env }, stdio: ['ignore', log, log] });
  child.unref();
  return child;
}

export async function waitForFile(file: string, ms = 20_000) {
  for (const end = Date.now() + ms; Date.now() < end; ) {
    if (fs.existsSync(file)) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`${file} did not appear`);
}

/** A minimal client of the GUI service, for building the data set. */
async function client(url: string) {
  const socket = new WebSocket(url);
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  let next = 1;
  const pending = new Map<number, (message: { result?: unknown; error?: { message: string } }) => void>();
  socket.onmessage = (event) => {
    const message = JSON.parse(String(event.data));
    pending.get(message.id)?.(message);
    pending.delete(message.id);
  };
  const call = <T>(method: string, params: unknown = {}) =>
    new Promise<T>((resolve, reject) => {
      const id = next++;
      pending.set(id, (m) => (m.error ? reject(new Error(m.error.message)) : resolve(m.result as T)));
      socket.send(JSON.stringify({ id, method, params }));
    });
  return { call, close: () => socket.close() };
}

type Snapshot = { attention: { kind: string; turn_id?: string }[] };

/** Waits until the executor's turn after `previous` has ended and been captured. */
async function turnDone(call: <T>(m: string, p?: unknown) => Promise<T>, previous: string | null): Promise<string> {
  for (const end = Date.now() + 120_000; Date.now() < end; ) {
    const snapshot = await call<Snapshot>('snapshot');
    const stalled = snapshot.attention.find((a) => a.kind === 'stalled');
    if (stalled?.turn_id && stalled.turn_id !== previous) return stalled.turn_id;
    if (snapshot.attention.some((a) => a.kind === 'hold')) throw new Error(`a turn did not end normally: ${JSON.stringify(snapshot.attention)}`);
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error('a data set turn did not end in time');
}

export default async function setup() {
  if (!fs.existsSync(exe)) throw new Error(`build the backend first: cargo build --release -p lobotomyd (${exe})`);
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'lobotomy-bench-'));
  const repo = path.join(root, 'repo');
  fs.mkdirSync(repo);
  git(repo, 'init', '--quiet', '--initial-branch', 'main');
  git(repo, 'config', 'user.name', 'Bench');
  git(repo, 'config', 'user.email', 'bench@example.com');
  fs.writeFileSync(path.join(repo, 'README.md'), '# bench\n');
  git(repo, 'add', '.');
  git(repo, 'commit', '--quiet', '-m', 'initial');

  const data = path.join(root, 'project');
  const host = path.join(root, 'host');
  const port = await freePort();
  const backend = {
    exe,
    args: ['--data-dir', data, '--host-dir', host, '--repo', repo, '--port', String(port)],
    env: { LOBOTOMY_CODEX: JSON.stringify(['node', fakeCodex]), LOBOTOMY_CLAUDE: JSON.stringify(['node', fakeClaude]) },
  };
  startBackend({ backend, root });
  await waitForFile(path.join(data, 'backend.json'));
  const token = fs.readFileSync(path.join(host, 'gui-token'), 'utf8').trim();

  const started = Date.now();
  const { call, close } = await client(`ws://127.0.0.1:${port}/gui?token=${encodeURIComponent(token)}`);
  const args = { request_id: 'bench-task', title: FIRST_TASK, body: TURNS[0], criteria: '-', executor: 'Malkuth' };
  const task = (await call<{ id: string }>('command', { name: 'create_task', args })).id;
  let turn = await turnDone(call, null);
  for (const [i, body] of TURNS.slice(1).entries()) {
    const message = { request_id: `bench-${i}`, role: 'Malkuth', task_id: task, body };
    await call('command', { name: 'send_message', args: message });
    turn = await turnDone(call, turn);
  }
  const page = await call<{ items: { seq: number }[] }>('thread', { role: 'Malkuth', limit: 1 });
  close();
  const items = page.items[0]?.seq ?? 0;
  console.log(`bench data set: ${items} items in ${((Date.now() - started) / 1000).toFixed(1)} s`);
  const info: BenchInfo = { port, token, root, backend, items };
  fs.writeFileSync(infoFile, JSON.stringify(info));

  return async () => {
    const running = JSON.parse(fs.readFileSync(path.join(data, 'backend.json'), 'utf8')) as { pid: number };
    try {
      process.kill(running.pid);
    } catch {
      // Already gone.
    }
    // The retries wait for Windows to let go of the killed backend's files.
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
  };
}
