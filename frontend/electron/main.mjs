// Electron's main process (frontend.md §1): starts the backend, one for all projects, when it is
// not running, keeps the app in the tray when the window closes, and stops the organization when
// the user quits from the tray. It carries no business data.

import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, shell, Tray } from 'electron';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const dev = process.argv.includes('--dev');
const hostDir =
  process.env.LOBOTOMY_HOST_DIR ??
  (process.platform === 'win32'
    ? path.join(process.env.LOCALAPPDATA ?? os.homedir(), 'Lobotomy')
    : path.join(process.env.XDG_STATE_HOME ?? path.join(os.homedir(), '.local', 'state'), 'lobotomy'));
// Until there is an installer, the backend comes from the source tree.
const backendExe =
  process.env.LOBOTOMYD ??
  path.join(here, '..', '..', 'backend', 'target', 'debug', process.platform === 'win32' ? 'lobotomyd.exe' : 'lobotomyd');
// Where the running backend says it listens (frontend.md §1).
const infoPath = path.join(hostDir, 'backend.json');

let window = null;
let tray = null;
let quitting = false;

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}

function reachable(port) {
  return new Promise((resolve) => {
    const socket = net.connect({ host: '127.0.0.1', port }, () => {
      socket.end();
      resolve(true);
    });
    socket.on('error', () => resolve(false));
    socket.setTimeout(1000, () => {
      socket.destroy();
      resolve(false);
    });
  });
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** A start in progress: callers at the same time share it (#16). */
let starting = null;

/** The running backend, started if needed. It outlives this process. */
function ensureBackend() {
  starting ??= startBackend().finally(() => (starting = null));
  return starting;
}

async function startBackend() {
  const info = readJson(infoPath);
  if (info && (await reachable(info.port))) return info;
  fs.rmSync(infoPath, { force: true });
  fs.mkdirSync(hostDir, { recursive: true });
  const logPath = path.join(hostDir, 'backend.log');
  const log = fs.openSync(logPath, 'a');
  const child = spawn(backendExe, ['--host-dir', hostDir], {
    detached: true,
    stdio: ['ignore', log, log],
    windowsHide: true,
  });
  child.unref();
  for (let i = 0; i < 100; i++) {
    await sleep(200);
    const started = readJson(infoPath);
    if (started && (await reachable(started.port))) return started;
  }
  throw new Error(`后端没有启动，见 ${logPath}`);
}

function token() {
  return fs.readFileSync(path.join(hostDir, 'gui-token'), 'utf8').trim();
}

ipcMain.handle('lobotomy:connection', async () => {
  const { port } = await ensureBackend();
  return { url: `ws://127.0.0.1:${port}/gui`, token: token() };
});

// While the window reconnects: a backend that restarted listens on another port. One that is not
// running is not started here; the user decides that by restarting the app.
ipcMain.handle('lobotomy:find', async () => {
  const info = readJson(infoPath);
  if (!info || !(await reachable(info.port))) return null;
  return { url: `ws://127.0.0.1:${info.port}/gui`, token: token() };
});

ipcMain.handle('lobotomy:chooseRepo', async () => {
  const result = await dialog.showOpenDialog(window, { title: '选择仓库', properties: ['openDirectory'] });
  if (result.canceled || result.filePaths.length === 0) return null;
  return result.filePaths[0];
});

ipcMain.on('lobotomy:attention', (_event, count) => {
  const label = count > 0 ? `等你决定 ${count}` : '';
  window?.setTitle(count > 0 ? `(${count}) Lobotomy` : 'Lobotomy');
  tray?.setToolTip(label ? `Lobotomy · ${label}` : 'Lobotomy');
});

ipcMain.on('lobotomy:openExternal', (_event, url) => {
  if (/^(https?:|mailto:)/.test(url)) void shell.openExternal(url);
});

/** Quitting stops the organization (frontend.md §1): the backend interrupts running turns,
 *  waits for them, and exits. */
async function quit() {
  quitting = true;
  window?.setTitle('Lobotomy · 正在停止…');
  const info = readJson(infoPath);
  if (info && (await reachable(info.port))) {
    try {
      await new Promise((resolve, reject) => {
        const socket = new WebSocket(`ws://127.0.0.1:${info.port}/gui?token=${encodeURIComponent(token())}`);
        socket.onopen = () => socket.send(JSON.stringify({ id: 1, method: 'command', params: { name: 'shutdown', args: {} } }));
        // Pushes and ticks come on the same socket; only the reply to the request counts.
        socket.onmessage = (event) => {
          const message = JSON.parse(String(event.data));
          if (message.id !== 1) return;
          socket.close();
          if (message.error) reject(new Error(message.error.message));
          else resolve();
        };
        socket.onerror = () => reject(new Error('连不上后端'));
      });
      // The backend waits up to 30 s for interrupted turns before it exits.
      for (let i = 0; i < 200 && (await reachable(info.port)); i++) await sleep(200);
    } catch (e) {
      dialog.showErrorBox('Lobotomy', `没能停止后端：${e}`);
    }
  }
  app.quit();
}
// The tray's "退出" and the end-to-end tests call it.
globalThis.lobotomyQuit = quit;

/** A small round tray icon, drawn here so the app ships no image files yet. */
function trayIcon() {
  const size = 32;
  const rows = [];
  for (let y = 0; y < size; y++) {
    const row = Buffer.alloc(1 + size * 4);
    for (let x = 0; x < size; x++) {
      const d = Math.hypot(x - size / 2 + 0.5, y - size / 2 + 0.5);
      const alpha = Math.max(0, Math.min(1, size / 2 - 2 - d)) * 255;
      row.set([0x8a, 0x5a, 0x2b, alpha], 1 + x * 4);
    }
    rows.push(row);
  }
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (buf) => {
    let c = 0xffffffff;
    for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type), data]);
    const out = Buffer.alloc(8 + data.length + 4);
    out.writeUInt32BE(data.length, 0);
    body.copy(out, 4);
    out.writeUInt32BE(crc(body), 8 + data.length);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header.set([8, 6, 0, 0, 0], 8);
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', zlib.deflateSync(Buffer.concat(rows))),
    chunk('IEND', Buffer.alloc(0)),
  ]);
  return nativeImage.createFromBuffer(png);
}

function showWindow() {
  if (!window) return;
  window.show();
  window.focus();
}

function createWindow() {
  window = new BrowserWindow({
    width: 1320,
    height: 860,
    title: 'Lobotomy',
    icon: trayIcon(),
    webPreferences: { preload: path.join(here, 'preload.cjs'), contextIsolation: true, sandbox: true },
  });
  window.removeMenu();
  // Links never navigate the app window; they open in the system browser.
  window.webContents.setWindowOpenHandler(({ url }) => {
    if (/^(https?:|mailto:)/.test(url)) void shell.openExternal(url);
    return { action: 'deny' };
  });
  window.webContents.on('will-navigate', (event, url) => {
    if (!url.startsWith('http://127.0.0.1:5173') && !url.startsWith('file://')) event.preventDefault();
  });
  window.on('close', (event) => {
    if (!quitting) {
      event.preventDefault();
      window.hide();
    }
  });
  if (dev) void window.loadURL('http://127.0.0.1:5173');
  else void window.loadFile(path.join(here, '..', 'dist', 'index.html'));
}

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', showWindow);
  app.whenReady().then(() => {
    createWindow();
    tray = new Tray(trayIcon());
    tray.setToolTip('Lobotomy');
    tray.setContextMenu(
      Menu.buildFromTemplate([
        { label: '打开 Lobotomy', click: showWindow },
        { type: 'separator' },
        { label: '退出 Lobotomy（停止组织）', click: () => void quit() },
      ]),
    );
    tray.on('click', showWindow);
  });
  // Closing the window keeps the organization running in the tray.
  app.on('window-all-closed', () => {});
  // Quitting some other way (the OS, a test) leaves the backend running; it is independent.
  app.on('before-quit', () => {
    quitting = true;
  });
}
